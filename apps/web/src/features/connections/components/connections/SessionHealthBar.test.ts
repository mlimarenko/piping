import { describe, expect, it } from "vitest";

import type { ConnectionSessionHealth } from "@/features/connections/api";
import {
  formatRouteLatency,
  getHealthRouteEdgeEndpoints,
  getHealthTopology,
  getWorstHealthLeg,
  normalizeHealthRouteMode,
  normalizeHealthState,
  projectHealthSnapshot,
  projectViewerHealthState,
  withClientBackendLatency,
} from "./sessionHealthProjection";

function sampleHealth(): ConnectionSessionHealth {
  return {
    session_id: "session-1",
    connection_type: "ssh",
    route_mode: "proxied",
    state: "ok",
    reason_code: null,
    worst_leg_id: null,
    proxy_node_name: "pxm-proxy01",
    updated_at: new Date(0).toISOString(),
    stale_after_ms: 7_500,
    legs: [
      {
        id: "client_backend",
        state: "unknown",
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
        state: "degraded",
        latency_ms: 90,
        rx_bps: 100,
        tx_bps: 200,
        rx_bytes: 10,
        tx_bytes: 20,
        quality_label: "warning",
        quality_score: 75,
        message: null,
      },
      {
        id: "proxy_target",
        state: "ok",
        latency_ms: 20,
        rx_bps: 300,
        tx_bps: 400,
        rx_bytes: 30,
        tx_bytes: 40,
        quality_label: "good",
        quality_score: 100,
        message: null,
      },
    ],
  };
}

describe("SessionHealthBar projection", () => {
  it("normalizes future backend states to unknown", () => {
    expect(normalizeHealthState("maintenance")).toBe("unknown");
    expect(normalizeHealthState("ok")).toBe("ok");
  });

  it("keeps backend top-level state as the compact footer source of truth", () => {
    const view = projectHealthSnapshot(sampleHealth(), 1_000, 2_000);

    expect(view.health.legs[0]?.state).toBe("unknown");
    expect(view.state).toBe("ok");
    expect(view.health.worst_leg_id).toBeNull();
  });

  it("does not keep the footer connecting after the backend reports an open route", () => {
    const health = sampleHealth();
    health.state = "ok";
    health.legs = health.legs.map((leg) => ({ ...leg, state: "connecting" }));

    const view = projectHealthSnapshot(health, 1_000, 2_000);

    expect(view.state).toBe("ok");
    expect(view.health.worst_leg_id).toBeNull();
  });

  it("does not keep the footer connecting after route health is open", () => {
    const health = sampleHealth();
    health.state = "ok";
    health.legs = health.legs.map((leg) => ({ ...leg, state: "ok" }));

    const view = projectHealthSnapshot(health, 1_000, 2_000, "connecting");

    expect(view.state).toBe("ok");
    expect(view.health.state).toBe("ok");
    expect(view.health.worst_leg_id).toBeNull();
  });

  it("keeps the footer connecting while route health is not open yet", () => {
    const health = sampleHealth();
    health.state = "unknown";
    health.legs = health.legs.map((leg) => ({ ...leg, state: "unknown" }));

    const view = projectHealthSnapshot(health, 1_000, 2_000, "connecting");

    expect(view.state).toBe("connecting");
    expect(view.health.state).toBe("connecting");
    expect(view.health.worst_leg_id).toBeNull();
  });

  it("lets connected viewers use the backend route state", () => {
    expect(projectViewerHealthState("ok", "connected")).toBe("ok");
    expect(projectViewerHealthState("failed", "connected")).toBe("failed");
  });

  it("lets known route health override stale viewer startup state", () => {
    expect(projectViewerHealthState("ok", "connecting")).toBe("ok");
    expect(projectViewerHealthState("degraded", "reconnecting")).toBe("degraded");
    expect(projectViewerHealthState("unknown", "connecting")).toBe("connecting");
  });

  it("does not hide route failures behind viewer reconnecting", () => {
    expect(projectViewerHealthState("failed", "reconnecting")).toBe("failed");
    expect(projectViewerHealthState("disconnected", "connecting")).toBe("disconnected");
  });

  it("does not hide stale health behind a connecting viewer", () => {
    const health = sampleHealth();
    health.state = "ok";
    health.legs = health.legs.map((leg) => ({ ...leg, state: "ok" }));

    const view = projectHealthSnapshot(health, 1_000, 12_000, "connecting");

    expect(view.stale).toBe(true);
    expect(view.state).toBe("unknown");
    expect(view.health.state).toBe("unknown");
    expect(view.health.worst_leg_id).toBeNull();
  });

  it("falls back to a matching worst leg for non-ok backend states", () => {
    const health = sampleHealth();
    health.state = "degraded";

    const view = projectHealthSnapshot(health, 1_000, 2_000);

    expect(view.state).toBe("degraded");
    expect(view.health.worst_leg_id).toBe("backend_proxy");
  });

  it("does not invent a worst leg when no non-ok leg matches the backend state", () => {
    const health = sampleHealth();
    health.state = "failed";
    health.worst_leg_id = "retired_leg";
    health.legs = health.legs.map((leg) => ({ ...leg, state: "ok" }));

    const view = projectHealthSnapshot(health, 1_000, 2_000);

    expect(view.state).toBe("failed");
    expect(view.health.worst_leg_id).toBeNull();
    expect(getWorstHealthLeg(view.health)).toBeNull();
  });

  it("resolves the explicit worst leg for diagnosis copy", () => {
    const health = sampleHealth();
    health.legs[2] = { ...health.legs[2]!, state: "failed" };
    health.worst_leg_id = "proxy_target";

    expect(getWorstHealthLeg(health)?.id).toBe("proxy_target");
  });

  it("preserves a valid backend worst leg through projection", () => {
    const health = sampleHealth();
    health.legs[2] = { ...health.legs[2]!, state: "failed" };
    health.state = "failed";
    health.worst_leg_id = "proxy_target";

    const view = projectHealthSnapshot(health, 1_000, 2_000);

    expect(view.health.worst_leg_id).toBe("proxy_target");
    expect(view.state).toBe("failed");
  });

  it("marks the snapshot unknown when polling is stale", () => {
    const view = projectHealthSnapshot(sampleHealth(), 1_000, 12_000);

    expect(view.stale).toBe(true);
    expect(view.state).toBe("unknown");
    expect(view.health.state).toBe("unknown");
    expect(view.health.legs[0]?.state).toBe("unknown");
    expect(view.health.worst_leg_id).toBeNull();
  });

  it("formats total route latency as an exact sum when all legs are measured", () => {
    const health = sampleHealth();
    health.legs[0] = { ...health.legs[0]!, latency_ms: 7 };

    expect(formatRouteLatency(health.legs)).toBe("117 ms");
  });

  it("uses the active browser health request RTT for the You-Piping leg when backend latency is absent", () => {
    const measured = withClientBackendLatency(sampleHealth(), 14.4);

    expect(measured.legs[0]?.latency_ms).toBe(14);
    expect(formatRouteLatency(measured.legs)).toBe("124 ms");
  });

  it("keeps sub-millisecond browser RTT visible as one millisecond", () => {
    const measured = withClientBackendLatency(sampleHealth(), 0.4);

    expect(measured.legs[0]?.latency_ms).toBe(1);
    expect(formatRouteLatency(measured.legs)).toBe("111 ms");
  });

  it("replaces zero You-Piping placeholders with the browser RTT", () => {
    const health = sampleHealth();
    health.legs[0] = { ...health.legs[0]!, latency_ms: 0 };

    const measured = withClientBackendLatency(health, 6);

    expect(measured.legs[0]?.latency_ms).toBe(6);
    expect(formatRouteLatency(measured.legs)).toBe("116 ms");
  });

  it("preserves a backend-provided You-Piping latency over the browser fallback", () => {
    const health = sampleHealth();
    health.legs[0] = { ...health.legs[0]!, latency_ms: 7 };

    const measured = withClientBackendLatency(health, 14);

    expect(measured.legs[0]?.latency_ms).toBe(7);
    expect(formatRouteLatency(measured.legs)).toBe("117 ms");
  });

  it("formats total route latency as the sum of measured legs when some legs are missing", () => {
    expect(formatRouteLatency(sampleHealth().legs)).toBe("110 ms");
  });

  it("omits total route latency when no leg latency is known", () => {
    const health = sampleHealth();
    health.legs = health.legs.map((leg) => ({ ...leg, latency_ms: null }));

    expect(formatRouteLatency(health.legs)).toBe("-");
  });

  it("does not treat zero latency placeholders as measured route latency", () => {
    const health = sampleHealth();
    health.legs = health.legs.map((leg) => ({ ...leg, latency_ms: 0 }));

    expect(formatRouteLatency(health.legs)).toBe("-");
  });

  it("projects proxied health as user-piping-proxy-host topology", () => {
    expect(getHealthTopology(sampleHealth()).map((edge) => [edge.id, edge.from, edge.to])).toEqual([
      ["client_backend", "user", "piping"],
      ["backend_proxy", "piping", "proxy"],
      ["proxy_target", "proxy", "host"],
    ]);
  });

  it("projects direct health without proxy edges", () => {
    const health = sampleHealth();
    health.route_mode = "direct";
    health.legs = [
      health.legs[0]!,
      {
        id: "backend_target",
        state: "ok",
        latency_ms: 12,
        rx_bps: 10,
        tx_bps: 20,
        rx_bytes: 100,
        tx_bytes: 200,
        quality_label: "good",
        quality_score: 100,
        message: null,
      },
    ];

    expect(getHealthTopology(health).map((edge) => [edge.id, edge.from, edge.to])).toEqual([
      ["client_backend", "user", "piping"],
      ["backend_target", "piping", "host"],
    ]);
  });

  it("uses the same endpoint source for dynamic leg labels", () => {
    expect(getHealthRouteEdgeEndpoints("backend_proxy")).toEqual({
      from: "piping",
      to: "proxy",
    });
    expect(getHealthRouteEdgeEndpoints("future_leg")).toBeNull();
  });

  it("uses route_mode as the active route source even with extra stale legs", () => {
    const health = sampleHealth();
    health.route_mode = "direct";

    expect(getHealthTopology(health).map((edge) => edge.id)).toEqual([
      "client_backend",
      "backend_target",
    ]);
  });

  it("normalizes unknown route modes to direct", () => {
    expect(normalizeHealthRouteMode("proxied")).toBe("proxied");
    expect(normalizeHealthRouteMode("something-else")).toBe("direct");
  });
});
