import { describe, expect, it } from "vitest";

import { formatAuditTimestamp, formatAuditTimestampUtc } from "./auditTime";

describe("audit timestamp formatting", () => {
  it("formats UTC audit timestamps in the requested display time zone", () => {
    const value = "2026-06-16T09:20:41Z";

    expect(formatAuditTimestamp(value, "ru-RU", "Europe/Moscow")).toContain("12:20:41");
  });

  it("keeps the raw UTC timestamp available for audit precision", () => {
    expect(formatAuditTimestampUtc("2026-06-16T09:20:41Z")).toBe("2026-06-16T09:20:41.000Z");
  });

  it("uses an empty dash for missing or invalid timestamps", () => {
    expect(formatAuditTimestamp(undefined, "ru-RU", "Europe/Moscow")).toBe("—");
    expect(formatAuditTimestamp("not-a-date", "ru-RU", "Europe/Moscow")).toBe("—");
    expect(formatAuditTimestampUtc("not-a-date")).toBe("—");
  });
});

