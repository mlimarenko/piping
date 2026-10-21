import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { ConnectionSessionHealth } from "@/features/connections/api";
import type { ViewerSessionStatus } from "@/shared/api/types";
import {
  CONNECTION_LOST_POLL_THRESHOLD,
  isVisualSessionRouteDown,
  useConnectionLossWatch,
} from "./connectionLossWatch";

function makeHealth(
  clientBackendState: string,
  overrides: Partial<ConnectionSessionHealth> = {},
): ConnectionSessionHealth {
  return {
    session_id: "s1",
    connection_type: "rdp",
    route_mode: "proxied",
    state: "ok",
    reason_code: null,
    worst_leg_id: null,
    proxy_node_name: null,
    updated_at: new Date(0).toISOString(),
    stale_after_ms: 7_500,
    legs: [
      {
        id: "client_backend",
        state: clientBackendState,
        latency_ms: null,
        rx_bps: null,
        tx_bps: null,
        rx_bytes: 0,
        tx_bytes: 0,
        quality_label: null,
        quality_score: null,
        message: null,
      },
      {
        id: "backend_proxy",
        state: "ok",
        latency_ms: null,
        rx_bps: null,
        tx_bps: null,
        rx_bytes: 0,
        tx_bytes: 0,
        quality_label: null,
        quality_score: null,
        message: null,
      },
      {
        id: "proxy_target",
        state: "ok",
        latency_ms: null,
        rx_bps: null,
        tx_bps: null,
        rx_bytes: 0,
        tx_bytes: 0,
        quality_label: null,
        quality_score: null,
        message: null,
      },
    ],
    ...overrides,
  };
}

describe("isVisualSessionRouteDown", () => {
  it("is true when the whole route is disconnected or failed", () => {
    expect(isVisualSessionRouteDown(makeHealth("ok", { state: "disconnected" }))).toBe(true);
    expect(isVisualSessionRouteDown(makeHealth("ok", { state: "failed" }))).toBe(true);
  });

  it("is true when any active route leg is disconnected or failed", () => {
    expect(isVisualSessionRouteDown(makeHealth("disconnected"))).toBe(true);
    expect(isVisualSessionRouteDown(makeHealth("failed"))).toBe(true);
    expect(isVisualSessionRouteDown(makeHealth("ok", {
      worst_leg_id: "proxy_target",
      legs: makeHealth("ok").legs.map((leg) =>
        leg.id === "proxy_target" ? { ...leg, state: "failed" } : leg,
      ),
    }))).toBe(true);
  });

  it("is false when active route legs are healthy, connecting, or unknown", () => {
    expect(isVisualSessionRouteDown(makeHealth("ok"))).toBe(false);
    expect(isVisualSessionRouteDown(makeHealth("connecting", { state: "connecting" }))).toBe(false);
    expect(isVisualSessionRouteDown(makeHealth("unknown", { state: "unknown" }))).toBe(false);
  });
});

describe("useConnectionLossWatch", () => {
  interface Props {
    health: ConnectionSessionHealth | undefined;
    dataUpdatedAt: number;
    viewerStatus: ViewerSessionStatus | undefined;
  }
  const setup = (onLost: () => void, initial: Props) =>
    renderHook(
      (p: Props) => useConnectionLossWatch(p.health, p.dataUpdatedAt, p.viewerStatus, onLost),
      { initialProps: initial },
    );

  it("fires once after the debounce threshold of consecutive down polls", () => {
    const onLost = vi.fn();
    const { rerender } = setup(onLost, {
      health: makeHealth("ok"),
      dataUpdatedAt: 1,
      viewerStatus: "connected",
    });
    expect(onLost).not.toHaveBeenCalled();
    for (let i = 0; i < CONNECTION_LOST_POLL_THRESHOLD; i += 1) {
      rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 10 + i, viewerStatus: "connected" });
    }
    expect(onLost).toHaveBeenCalledTimes(1);
    // Further down polls in the same episode must not re-fire.
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 100, viewerStatus: "connected" });
    expect(onLost).toHaveBeenCalledTimes(1);
  });

  it("does not fire before the threshold", () => {
    const onLost = vi.fn();
    const { rerender } = setup(onLost, {
      health: makeHealth("ok"),
      dataUpdatedAt: 1,
      viewerStatus: "connected",
    });
    for (let i = 0; i < CONNECTION_LOST_POLL_THRESHOLD - 1; i += 1) {
      rerender({ health: makeHealth("failed"), dataUpdatedAt: 10 + i, viewerStatus: "connected" });
    }
    expect(onLost).not.toHaveBeenCalled();
  });

  it("resets the streak when a healthy poll interrupts (transient blip)", () => {
    const onLost = vi.fn();
    const { rerender } = setup(onLost, {
      health: makeHealth("disconnected"),
      dataUpdatedAt: 1,
      viewerStatus: "connected",
    });
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 2, viewerStatus: "connected" });
    rerender({ health: makeHealth("ok"), dataUpdatedAt: 3, viewerStatus: "connected" });
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 4, viewerStatus: "connected" });
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 5, viewerStatus: "connected" });
    expect(onLost).not.toHaveBeenCalled();
  });

  it("does not fire while the viewer is not in a connected state", () => {
    const onLost = vi.fn();
    const { rerender } = setup(onLost, {
      health: makeHealth("disconnected"),
      dataUpdatedAt: 1,
      viewerStatus: "connecting",
    });
    for (let i = 0; i < CONNECTION_LOST_POLL_THRESHOLD + 2; i += 1) {
      rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 10 + i, viewerStatus: "reconnecting" });
    }
    expect(onLost).not.toHaveBeenCalled();
  });

  it("counts once per snapshot, ignoring re-renders with the same dataUpdatedAt", () => {
    const onLost = vi.fn();
    const { rerender } = setup(onLost, {
      health: makeHealth("disconnected"),
      dataUpdatedAt: 5,
      viewerStatus: "connected",
    });
    // Parent re-renders without a fresh poll must not advance the streak.
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 5, viewerStatus: "connected" });
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 5, viewerStatus: "connected" });
    rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 5, viewerStatus: "connected" });
    expect(onLost).not.toHaveBeenCalled();
  });

  it("re-arms for a new connected episode after the viewer leaves connected", () => {
    const onLost = vi.fn();
    const { rerender } = setup(onLost, {
      health: makeHealth("ok"),
      dataUpdatedAt: 1,
      viewerStatus: "connected",
    });
    for (let i = 0; i < CONNECTION_LOST_POLL_THRESHOLD; i += 1) {
      rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 10 + i, viewerStatus: "connected" });
    }
    expect(onLost).toHaveBeenCalledTimes(1);
    // Reconnect cycle re-arms the watcher.
    rerender({ health: makeHealth("ok"), dataUpdatedAt: 20, viewerStatus: "reconnecting" });
    rerender({ health: makeHealth("ok"), dataUpdatedAt: 21, viewerStatus: "connected" });
    for (let i = 0; i < CONNECTION_LOST_POLL_THRESHOLD; i += 1) {
      rerender({ health: makeHealth("disconnected"), dataUpdatedAt: 30 + i, viewerStatus: "connected" });
    }
    expect(onLost).toHaveBeenCalledTimes(2);
  });
});
