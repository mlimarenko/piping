import { useEffect, useRef } from "react";

import type { ConnectionSessionHealth } from "@/features/connections/api";
import type { ViewerSessionStatus } from "@/shared/api/types";
import { getHealthTopology, normalizeHealthState } from "./sessionHealthProjection";

// Consecutive health snapshots (≈1s polls) the authoritative relay must report
// the user↔backend leg down before a viewer that still believes it is
// "connected" is treated as a dead session. Debounced so one transient blip
// never trips the reconnect overlay.
export const CONNECTION_LOST_POLL_THRESHOLD = 3;

/**
 * True when the authoritative route health reports a live visual session as
 * non-usable. Guacamole/noVNC canvases can keep showing the last remote frame
 * when any relay segment dies, not just the browser↔Piping leg, so recovery
 * must follow the whole active route.
 */
export function isVisualSessionRouteDown(health: ConnectionSessionHealth): boolean {
  const routeState = normalizeHealthState(health.state);
  if (routeState === "disconnected" || routeState === "failed") return true;

  return getHealthTopology(health).some((edge) => {
    const state = normalizeHealthState(edge.leg?.state);
    return state === "disconnected" || state === "failed";
  });
}

/**
 * Fires `onConnectionLost` exactly once when a visual viewer that still believes
 * it is `connected` has its route health reported down for
 * {@link CONNECTION_LOST_POLL_THRESHOLD} consecutive snapshots. Counts once per
 * snapshot (keyed on `dataUpdatedAt`) so an unstable `onConnectionLost` identity
 * cannot inflate the streak, and re-arms whenever the viewer leaves the
 * `connected` state (so a later reconnect can be detected again).
 */
export function useConnectionLossWatch(
  health: ConnectionSessionHealth | undefined,
  dataUpdatedAt: number,
  viewerStatus: ViewerSessionStatus | undefined,
  onConnectionLost?: () => void,
): void {
  const streakRef = useRef(0);
  const firedRef = useRef(false);
  const lastProcessedRef = useRef(0);

  useEffect(() => {
    if (viewerStatus !== "connected") {
      streakRef.current = 0;
      firedRef.current = false;
      return;
    }
    if (!health || dataUpdatedAt === lastProcessedRef.current) return;
    lastProcessedRef.current = dataUpdatedAt;

    if (!isVisualSessionRouteDown(health)) {
      streakRef.current = 0;
      return;
    }
    streakRef.current += 1;
    if (streakRef.current >= CONNECTION_LOST_POLL_THRESHOLD && !firedRef.current) {
      firedRef.current = true;
      onConnectionLost?.();
    }
  }, [health, dataUpdatedAt, viewerStatus, onConnectionLost]);
}
