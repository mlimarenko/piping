import { describe, expect, it } from "vitest";
import type { TFunction } from "i18next";

import {
  clampToRetention,
  dayBounds,
  formatDuration,
  MAX_WINDOW_SECONDS,
  segmentsForDay,
  windowFromSpan,
  windowHasFootage,
  windowProblem,
  windowProblemMessage,
} from "./archiveWindow";

describe("windowProblem", () => {
  it("accepts a normal window", () => {
    const start = Date.parse("2026-07-25T04:00:00Z");
    expect(windowProblem(start, start + 60_000)).toBeNull();
  });

  it("rejects an inverted or empty window", () => {
    const start = Date.parse("2026-07-25T04:00:00Z");
    expect(windowProblem(start, start)).toBe("inverted");
    expect(windowProblem(start, start - 1000)).toBe("inverted");
  });

  it("rejects a window longer than the server accepts", () => {
    const start = Date.parse("2026-07-25T04:00:00Z");
    expect(windowProblem(start, start + MAX_WINDOW_SECONDS * 1000)).toBeNull();
    expect(windowProblem(start, start + (MAX_WINDOW_SECONDS + 1) * 1000)).toBe("too-long");
  });

  it("rejects unparseable timestamps rather than sending them", () => {
    expect(windowProblem(Number.NaN, Date.now())).toBe("invalid");
  });
});

describe("clampToRetention", () => {
  const retention = { oldest: "2026-07-25T00:00:00Z", newest: "2026-07-25T10:00:00Z" };

  it("keeps a window already inside the retained range", () => {
    const window = {
      startMs: Date.parse("2026-07-25T04:00:00Z"),
      endMs: Date.parse("2026-07-25T04:05:00Z"),
    };
    expect(clampToRetention(window, retention)).toEqual(window);
  });

  it("shifts a window that starts before the oldest footage, preserving its length", () => {
    const window = {
      startMs: Date.parse("2026-07-24T23:00:00Z"),
      endMs: Date.parse("2026-07-24T23:10:00Z"),
    };

    const clamped = clampToRetention(window, retention);

    expect(new Date(clamped.startMs).toISOString()).toBe("2026-07-25T00:00:00.000Z");
    expect(clamped.endMs - clamped.startMs).toBe(window.endMs - window.startMs);
  });

  it("pulls a window that runs past the newest footage back to the end", () => {
    const window = {
      startMs: Date.parse("2026-07-25T09:55:00Z"),
      endMs: Date.parse("2026-07-25T10:30:00Z"),
    };

    const clamped = clampToRetention(window, retention);

    expect(new Date(clamped.endMs).toISOString()).toBe("2026-07-25T10:00:00.000Z");
    expect(clamped.startMs).toBeLessThan(clamped.endMs);
  });

  /// A request entirely after the newest footage must still come back as a
  /// usable window: an inverted one reads as valid to every later check and is
  /// then sent to the device.
  it("keeps a window valid when the whole request sits past the newest footage", () => {
    const window = {
      startMs: Date.parse("2026-07-25T12:00:00Z"),
      endMs: Date.parse("2026-07-25T12:05:00Z"),
    };

    const clamped = clampToRetention(window, retention);

    expect(clamped.endMs).toBeGreaterThan(clamped.startMs);
    expect(new Date(clamped.endMs).toISOString()).toBe("2026-07-25T10:00:00.000Z");
    expect(clamped.endMs - clamped.startMs).toBe(window.endMs - window.startMs);
    expect(windowProblem(clamped.startMs, clamped.endMs)).toBeNull();
  });

  it("leaves a bound the device does not report untouched", () => {
    const window = {
      startMs: Date.parse("2026-07-20T00:00:00Z"),
      endMs: Date.parse("2026-07-20T00:05:00Z"),
    };

    expect(clampToRetention(window, { oldest: null, newest: null })).toEqual(window);
  });
});

describe("windowHasFootage", () => {
  const spans = [
    { start: "2026-07-25T04:00:00Z", end: "2026-07-25T05:00:00Z" },
    { start: "2026-07-25T06:00:00Z", end: "2026-07-25T07:00:00Z" },
  ];

  it("detects an overlapping window", () => {
    const window = {
      startMs: Date.parse("2026-07-25T04:30:00Z"),
      endMs: Date.parse("2026-07-25T04:40:00Z"),
    };
    expect(windowHasFootage(window, spans)).toBe(true);
  });

  it("detects a window that falls into a recording gap", () => {
    const window = {
      startMs: Date.parse("2026-07-25T05:10:00Z"),
      endMs: Date.parse("2026-07-25T05:20:00Z"),
    };
    expect(windowHasFootage(window, spans)).toBe(false);
  });

  /// A device that does not enumerate spans has told us nothing, which must not
  /// be read as "there is no footage" — that would hide playable archive.
  it("treats an unenumerated archive as possibly containing footage", () => {
    const window = {
      startMs: Date.parse("2026-07-25T05:10:00Z"),
      endMs: Date.parse("2026-07-25T05:20:00Z"),
    };
    expect(windowHasFootage(window, [])).toBe(true);
  });
});

describe("windowFromSpan", () => {
  it("uses the whole span when it fits the request limit", () => {
    const window = windowFromSpan({
      start: "2026-07-25T04:00:00Z",
      end: "2026-07-25T04:30:00Z",
    });

    expect(new Date(window.endMs).toISOString()).toBe("2026-07-25T04:30:00.000Z");
  });

  it("caps a span longer than one request may cover", () => {
    const window = windowFromSpan({
      start: "2026-07-25T00:00:00Z",
      end: "2026-07-25T23:00:00Z",
    });

    expect((window.endMs - window.startMs) / 1000).toBe(MAX_WINDOW_SECONDS);
  });
});

describe("windowProblemMessage", () => {
  /// The player renders whatever this returns verbatim, so a missing case would
  /// silently show no explanation for a window the UI has already refused.
  const t = ((key: string, options?: unknown) =>
    typeof options === "string" ? options : `${key}`) as unknown as TFunction;

  it("explains every way a window can be rejected", () => {
    for (const problem of ["inverted", "too-long", "invalid"] as const) {
      expect(windowProblemMessage(problem, t)).toBeTruthy();
    }
  });

  it("distinguishes the three rejections from each other", () => {
    const messages = (["inverted", "too-long", "invalid"] as const).map((problem) =>
      windowProblemMessage(problem, t),
    );
    expect(new Set(messages).size).toBe(3);
  });

  it("says nothing for a usable window", () => {
    expect(windowProblemMessage(null, t)).toBeNull();
  });
});

describe("dayBounds", () => {
  it("returns local midnight through the next local midnight", () => {
    const noon = new Date(2026, 6, 25, 12, 30).getTime();
    const bounds = dayBounds(noon);
    expect(bounds.startMs).toBe(new Date(2026, 6, 25, 0, 0, 0, 0).getTime());
    expect(bounds.endMs).toBe(new Date(2026, 6, 26, 0, 0, 0, 0).getTime());
  });
});

describe("segmentsForDay", () => {
  const { startMs: dayStartMs, endMs: dayEndMs } = dayBounds(
    new Date(2026, 6, 25, 12).getTime(),
  );

  it("positions a span fully inside the day as a fraction of it", () => {
    const span = {
      start: new Date(2026, 6, 25, 6).toISOString(),
      end: new Date(2026, 6, 25, 18).toISOString(),
    };
    const segments = segmentsForDay([span], dayStartMs, dayEndMs);
    expect(segments).toHaveLength(1);
    const segment = segments[0]!;
    expect(segment.startFraction).toBeCloseTo(0.25);
    expect(segment.endFraction).toBeCloseTo(0.75);
  });

  it("clips a span that starts before the day to the day boundary", () => {
    const span = {
      start: new Date(2026, 6, 24, 12).toISOString(),
      end: new Date(2026, 6, 25, 6).toISOString(),
    };
    const segments = segmentsForDay([span], dayStartMs, dayEndMs);
    expect(segments).toHaveLength(1);
    const segment = segments[0]!;
    expect(segment.startFraction).toBe(0);
    expect(segment.endFraction).toBeCloseTo(0.25);
  });

  it("clips a span that ends after the day to the day boundary", () => {
    const span = {
      start: new Date(2026, 6, 25, 18).toISOString(),
      end: new Date(2026, 6, 26, 6).toISOString(),
    };
    const segments = segmentsForDay([span], dayStartMs, dayEndMs);
    expect(segments).toHaveLength(1);
    const segment = segments[0]!;
    expect(segment.startFraction).toBeCloseTo(0.75);
    expect(segment.endFraction).toBe(1);
  });

  it("drops a span that does not overlap the day at all", () => {
    const span = {
      start: new Date(2026, 6, 20, 0).toISOString(),
      end: new Date(2026, 6, 20, 1).toISOString(),
    };
    expect(segmentsForDay([span], dayStartMs, dayEndMs)).toEqual([]);
  });

  it("drops a span with an unparseable timestamp instead of producing NaN fractions", () => {
    const span = { start: "not-a-date", end: new Date(2026, 6, 25, 6).toISOString() };
    expect(segmentsForDay([span], dayStartMs, dayEndMs)).toEqual([]);
  });
});

describe("formatDuration", () => {
  it("formats seconds, minutes and hours", () => {
    expect(formatDuration(45)).toBe("45s");
    expect(formatDuration(125)).toBe("2:05");
    expect(formatDuration(3725)).toBe("1:02:05");
  });

  it("renders a dash for a non-duration", () => {
    expect(formatDuration(0)).toBe("—");
    expect(formatDuration(Number.NaN)).toBe("—");
  });
});
