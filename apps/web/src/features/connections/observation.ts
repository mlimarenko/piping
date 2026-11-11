import { API_BASE_URL } from "@/shared/api/client";
import type { ActiveSession, SessionProtocol } from "@/shared/api/types";
import {
  readBrowserLocalSetting,
  removeBrowserLocalSetting,
  writeBrowserLocalSetting,
} from "@/shared/settings";

/** Protocols with a server observe relay; CCTV is a client-side console. */
export type ObservationProtocol = "ssh" | "rdp" | "vnc";

const OBSERVATION_TRANSPORT: Record<ObservationProtocol, "terminal" | "guacamole"> = {
  ssh: "terminal",
  rdp: "guacamole",
  vnc: "guacamole",
};

const OBSERVATION_HANDOFF_PREFIX = "connections.observe.";
const OBSERVATION_WINDOW_PATH_PREFIX = "/connections/observe/";

export function isObservationProtocol(value: unknown): value is ObservationProtocol {
  return value === "ssh" || value === "rdp" || value === "vnc";
}

/**
 * Validate the server capability and return the same-origin observe socket.
 *
 * The UI connects only to the address the backend named, only for the selected
 * session, and only over the relay transport that matches the protocol.
 */
export function observationWebsocketUrl(
  capability: ActiveSession["observe"],
  sessionId: string,
  protocol: SessionProtocol,
): string | null {
  if (!capability || typeof capability.allowed !== "boolean" || !capability.allowed) return null;
  if (!isObservationProtocol(protocol)) return null;
  if (capability.transport !== OBSERVATION_TRANSPORT[protocol]) return null;
  if (typeof capability.websocket_url !== "string" || !capability.websocket_url) return null;
  try {
    const origin = new URL(API_BASE_URL || window.location.origin, window.location.origin);
    const url = new URL(capability.websocket_url, origin);
    const expectedProtocol = origin.protocol === "https:" ? "wss:" : "ws:";
    if (
      ![origin.protocol, expectedProtocol].includes(url.protocol) ||
      url.host !== origin.host ||
      url.username ||
      url.password ||
      url.search ||
      url.hash ||
      url.pathname !== `/api/v1/ws/observe/${encodeURIComponent(sessionId)}`
    ) {
      return null;
    }
    url.protocol = expectedProtocol;
    return url.href;
  } catch {
    return null;
  }
}

/**
 * Snapshot of one observe session handed to a separate window. The new window
 * reads it from local storage, so a reload keeps the same read-only stream
 * without re-querying the session list.
 */
export interface ObservationHandoff {
  sessionId: string;
  hostId: string;
  hostName: string;
  userLabel?: string;
  protocol: ObservationProtocol;
  websocketUrl: string;
}

export function observationHandoffFor(
  session: ActiveSession,
  websocketUrl: string,
): ObservationHandoff | null {
  if (!isObservationProtocol(session.protocol)) return null;
  const userLabel = session.userDisplayName ?? session.userEmail;
  return {
    sessionId: session.id,
    hostId: session.hostId,
    hostName: session.hostName ?? session.hostId,
    ...(userLabel ? { userLabel } : {}),
    protocol: session.protocol,
    websocketUrl,
  };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function normalizeObservationHandoff(value: unknown): ObservationHandoff | null {
  if (!isRecord(value)) return null;
  const { sessionId, hostId, hostName, userLabel, protocol, websocketUrl } = value;
  if (typeof sessionId !== "string" || sessionId.length === 0) return null;
  if (typeof hostId !== "string" || hostId.length === 0) return null;
  if (typeof hostName !== "string" || hostName.length === 0) return null;
  if (userLabel !== undefined && (typeof userLabel !== "string" || userLabel.length === 0)) return null;
  if (!isObservationProtocol(protocol)) return null;

  const validatedUrl = observationWebsocketUrl(
    {
      allowed: true,
      reason: null,
      websocket_url: typeof websocketUrl === "string" ? websocketUrl : null,
      transport: OBSERVATION_TRANSPORT[protocol],
    },
    sessionId,
    protocol,
  );
  if (!validatedUrl) return null;

  return {
    sessionId,
    hostId,
    hostName,
    ...(typeof userLabel === "string" ? { userLabel } : {}),
    protocol,
    websocketUrl: validatedUrl,
  };
}

export function observationWindowPath(sessionId: string): string {
  const base = import.meta.env.BASE_URL;
  const basePath = base && base !== "/" ? base.replace(/\/$/, "") : "";
  return `${basePath}${OBSERVATION_WINDOW_PATH_PREFIX}${encodeURIComponent(sessionId)}`;
}

export function readObservationHandoff(sessionId: string): ObservationHandoff | null {
  return readBrowserLocalSetting<ObservationHandoff | null>(
    `${OBSERVATION_HANDOFF_PREFIX}${sessionId}`,
    null,
    normalizeObservationHandoff,
  );
}

export function writeObservationHandoff(handoff: ObservationHandoff): void {
  writeBrowserLocalSetting<ObservationHandoff | null>(
    `${OBSERVATION_HANDOFF_PREFIX}${handoff.sessionId}`,
    handoff,
    normalizeObservationHandoff,
  );
}

export function removeObservationHandoff(sessionId: string): void {
  removeBrowserLocalSetting(`${OBSERVATION_HANDOFF_PREFIX}${sessionId}`);
}

/**
 * Store the handoff and open the read-only viewer in a separate window.
 * Returns false when the browser blocked the window; the stale handoff is
 * dropped so a later retry cannot pick up the previous session.
 */
export function openObservationWindow(handoff: ObservationHandoff): boolean {
  if (typeof window === "undefined") return false;
  writeObservationHandoff(handoff);
  const opened = window.open(observationWindowPath(handoff.sessionId), "_blank");
  if (!opened) {
    removeObservationHandoff(handoff.sessionId);
    return false;
  }
  try {
    opened.opener = null;
  } catch {
    // Cross-origin browser policies may restrict a freshly opened window.
  }
  return true;
}
