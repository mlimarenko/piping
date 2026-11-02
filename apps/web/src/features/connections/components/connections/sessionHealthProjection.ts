import type {
  ConnectionSessionHealth,
  ConnectionSessionHealthLeg,
} from "@/features/connections/api";

export type NormalizedHealthState =
  | "connecting"
  | "ok"
  | "degraded"
  | "disconnected"
  | "failed"
  | "unknown";

export interface HealthViewModel {
  health: ConnectionSessionHealth;
  state: NormalizedHealthState;
  stale: boolean;
}

export type HealthRouteMode = "direct" | "proxied";

export type HealthRouteNodeId = "user" | "piping" | "proxy" | "host";

export type HealthViewerStatus =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "disconnected"
  | "failed"
  | "idle";

export type HealthRouteEdgeId =
  | "client_backend"
  | "backend_proxy"
  | "proxy_target"
  | "backend_target";

export interface HealthTopologyEdge {
  id: HealthRouteEdgeId;
  from: HealthRouteNodeId;
  to: HealthRouteNodeId;
  leg: ConnectionSessionHealthLeg | null;
}

export function normalizeHealthState(value: string | null | undefined): NormalizedHealthState {
  if (
    value === "connecting" ||
    value === "ok" ||
    value === "degraded" ||
    value === "disconnected" ||
    value === "failed" ||
    value === "unknown"
  ) {
    return value;
  }
  return "unknown";
}

const EDGE_ENDPOINTS: Record<HealthRouteEdgeId, { from: HealthRouteNodeId; to: HealthRouteNodeId }> = {
  client_backend: { from: "user", to: "piping" },
  backend_proxy: { from: "piping", to: "proxy" },
  proxy_target: { from: "proxy", to: "host" },
  backend_target: { from: "piping", to: "host" },
};

const DIRECT_EDGE_IDS = ["client_backend", "backend_target"] satisfies HealthRouteEdgeId[];
const PROXIED_EDGE_IDS = ["client_backend", "backend_proxy", "proxy_target"] satisfies HealthRouteEdgeId[];

export function normalizeHealthRouteMode(value: string | null | undefined): HealthRouteMode {
  return value === "proxied" ? "proxied" : "direct";
}

export function getWorstHealthLeg(
  health: Pick<ConnectionSessionHealth, "legs" | "worst_leg_id">,
): ConnectionSessionHealthLeg | null {
  const explicitWorst = health.worst_leg_id
    ? health.legs.find((leg) => leg.id === health.worst_leg_id)
    : null;
  return explicitWorst ?? null;
}

export function getHealthTopology(
  health: Pick<ConnectionSessionHealth, "legs" | "route_mode">,
): HealthTopologyEdge[] {
  const edgeIds = normalizeHealthRouteMode(health.route_mode) === "proxied"
    ? PROXIED_EDGE_IDS
    : DIRECT_EDGE_IDS;
  const legsById = new Map(health.legs.map((leg) => [leg.id, leg]));

  return edgeIds.map((id) => ({
    id,
    ...EDGE_ENDPOINTS[id],
    leg: legsById.get(id) ?? null,
  }));
}

export function getHealthRouteEdgeEndpoints(
  edgeId: string,
): { from: HealthRouteNodeId; to: HealthRouteNodeId } | null {
  return Object.prototype.hasOwnProperty.call(EDGE_ENDPOINTS, edgeId)
    ? EDGE_ENDPOINTS[edgeId as HealthRouteEdgeId]
    : null;
}

function resolveWorstLegId(
  health: Pick<ConnectionSessionHealth, "legs" | "worst_leg_id">,
  state: NormalizedHealthState,
): string | null {
  if (state === "ok") return null;

  const explicitWorst = health.worst_leg_id
    ? health.legs.find((leg) => leg.id === health.worst_leg_id) ?? null
    : null;
  if (explicitWorst) return explicitWorst.id;

  const matchingStateLeg = health.legs.find((leg) => normalizeHealthState(leg.state) === state);
  return matchingStateLeg && normalizeHealthState(matchingStateLeg.state) !== "ok"
    ? matchingStateLeg.id
    : null;
}

export function projectHealthSnapshot(
  health: ConnectionSessionHealth,
  dataUpdatedAt: number,
  nowMs: number,
  viewerStatus?: HealthViewerStatus,
): HealthViewModel {
  const staleAfterMs = Math.max(1_000, health.stale_after_ms || 7_500);
  const stale = dataUpdatedAt > 0 && nowMs - dataUpdatedAt > staleAfterMs;
  const routeState = stale ? "unknown" : normalizeHealthState(health.state);
  const state = stale ? routeState : projectViewerHealthState(routeState, viewerStatus);
  return {
    health: {
      ...health,
      state,
      worst_leg_id: stale ? null : resolveWorstLegId(health, state),
    },
    state,
    stale,
  };
}

export function projectViewerHealthState(
  routeState: NormalizedHealthState,
  viewerStatus?: HealthViewerStatus,
): NormalizedHealthState {
  if (!viewerStatus || viewerStatus === "connected") return routeState;

  if (viewerStatus === "failed") return "failed";
  if (viewerStatus === "disconnected" || viewerStatus === "idle") return "disconnected";

  if (viewerStatus === "connecting" || viewerStatus === "reconnecting") {
    return routeState === "unknown" || routeState === "connecting"
      ? "connecting"
      : routeState;
  }

  return routeState;
}

export function withClientBackendLatency(
  health: ConnectionSessionHealth,
  latencyMs: number | null | undefined,
): ConnectionSessionHealth {
  if (
    typeof latencyMs !== "number" ||
    !Number.isFinite(latencyMs) ||
    latencyMs <= 0
  ) {
    return health;
  }

  let changed = false;
  const normalizedLatencyMs = Math.max(1, Math.round(latencyMs));
  const legs = health.legs.map((leg) => {
    if (leg.id !== "client_backend") return leg;
    if (isMeasuredLatency(leg.latency_ms)) {
      return leg;
    }
    changed = true;
    return {
      ...leg,
      latency_ms: normalizedLatencyMs,
    };
  });

  return changed ? { ...health, legs } : health;
}

export function formatRouteLatency(legs: ConnectionSessionHealthLeg[]): string {
  let hasMeasuredLatency = false;
  let total = 0;

  for (const leg of legs) {
    if (isMeasuredLatency(leg.latency_ms)) {
      hasMeasuredLatency = true;
      total += leg.latency_ms;
    }
  }

  return hasMeasuredLatency ? formatLatency(total) : "-";
}

export function isMeasuredLatency(value: number | null | undefined): value is number {
  // Relay snapshots use 0 as a placeholder; real measured sub-ms RTT is normalized to 1 before rendering.
  return typeof value === "number" && Number.isFinite(value) && value > 0;
}

function formatLatency(value: number): string {
  return `${Math.max(1, Math.round(value))} ms`;
}
