import { describe, expect, it } from "vitest";

import { isTerminalStatus, isStopping, latestStatusMessage, statusPhase } from "./runPhase";
import type { PlaybookRunEvent } from "./api";

function event(partial: Partial<PlaybookRunEvent>): PlaybookRunEvent {
  return {
    id: partial.id ?? "evt",
    run_group_id: partial.run_group_id ?? "group-1",
    run_id: "run-1",
    host: partial.host ?? null,
    event_kind: partial.event_kind ?? "stdout",
    event_type: partial.event_type ?? "log",
    event_sequence: partial.event_sequence ?? 0,
    message: partial.message ?? null,
    payload: partial.payload ?? {},
    created_at: "2026-06-06T00:00:00Z",
  };
}

describe("statusPhase", () => {
  it("is pending while queued", () => {
    expect(statusPhase("queued")).toBe("pending");
  });

  it("is running while leased/dispatching/running", () => {
    expect(statusPhase("leased")).toBe("running");
    expect(statusPhase("dispatching")).toBe("running");
    expect(statusPhase("running")).toBe("running");
  });

  it("is finished for terminal statuses", () => {
    expect(statusPhase("succeeded")).toBe("finished");
    expect(statusPhase("failed")).toBe("finished");
    expect(statusPhase("canceled")).toBe("finished");
    expect(statusPhase("timed_out")).toBe("finished");
  });

  it("flags terminal statuses for poll gating", () => {
    expect(isTerminalStatus("running")).toBe(false);
    expect(isTerminalStatus("queued")).toBe(false);
    expect(isTerminalStatus("succeeded")).toBe(true);
    expect(isTerminalStatus("timed_out")).toBe(true);
  });
});

describe("isStopping", () => {
  it("is true when cancel_requested_at is set and status is non-terminal", () => {
    expect(isStopping("running", "2026-06-10T00:00:00Z")).toBe(true);
    expect(isStopping("leased", "2026-06-10T00:00:00Z")).toBe(true);
    expect(isStopping("dispatching", "2026-06-10T00:00:00Z")).toBe(true);
    expect(isStopping("queued", "2026-06-10T00:00:00Z")).toBe(true);
  });

  it("is false when cancel_requested_at is null", () => {
    expect(isStopping("running", null)).toBe(false);
    expect(isStopping("running", undefined)).toBe(false);
  });

  it("is false when status is terminal (cancel already landed)", () => {
    expect(isStopping("canceled", "2026-06-10T00:00:00Z")).toBe(false);
    expect(isStopping("succeeded", "2026-06-10T00:00:00Z")).toBe(false);
    expect(isStopping("failed", "2026-06-10T00:00:00Z")).toBe(false);
    expect(isStopping("timed_out", "2026-06-10T00:00:00Z")).toBe(false);
  });
});

describe("latestStatusMessage", () => {
  it("returns the latest status message", () => {
    const events = [
      event({ event_kind: "status", message: "dispatching", event_sequence: 1 }),
      event({ event_kind: "stdout", message: "noise", event_sequence: 2 }),
      event({ event_kind: "status", message: "running", event_sequence: 3 }),
    ];
    expect(latestStatusMessage(events)).toBe("running");
  });

  it("returns null when there is no status message", () => {
    expect(latestStatusMessage([event({ event_kind: "stdout", message: "x" })])).toBeNull();
  });
});
