import { isSessionWsConnectable } from "@/features/connections/adapters/ws/urls";
import {
  readBrowserLocalSetting,
  removeBrowserLocalSetting,
  writeBrowserLocalSetting,
} from "@/shared/settings";

const DETACHED_HANDOFF_PREFIX = "connections.detached.";

declare global {
  interface Window {
    __PIPING_DETACHED_HANDOFF__?: unknown;
    /**
     * Window-injected payload for a multi-session detached group window.
     * Mirrors the single-session injection: parent writes the array right
     * after `window.open`, child consumes it on first render. Per-session
     * `localStorage` handoffs are still written so a hard reload of the
     * group window restores each session through the same path the single
     * detached window uses.
     */
    __PIPING_DETACHED_GROUP_HANDOFF__?: unknown;
  }
}

export interface DetachedHandoff {
  sessionId: string;
  hostId: string;
  hostName: string;
  // `cctv` is a client-only console (no proxied websocket): its handoff
  // carries empty websocketUrl/token and the detached page renders the CCTV
  // console straight from `hostId` instead of resuming a backend session.
  protocol: "ssh" | "rdp" | "vnc" | "cctv";
  websocketUrl: string;
  token: string;
  username?: string;
}

export function normalizeDetachedHandoff(value: unknown): DetachedHandoff | null {
  if (typeof value !== "object" || value === null) return null;
  const parsed = value as Partial<DetachedHandoff>;
  if (
    typeof parsed.sessionId !== "string" ||
    typeof parsed.hostId !== "string" ||
    typeof parsed.hostName !== "string" ||
    typeof parsed.websocketUrl !== "string" ||
    typeof parsed.token !== "string" ||
    (parsed.protocol !== "ssh" &&
      parsed.protocol !== "rdp" &&
      parsed.protocol !== "vnc" &&
      parsed.protocol !== "cctv") ||
    (parsed.username !== undefined && typeof parsed.username !== "string")
  ) {
    return null;
  }
  if (parsed.protocol !== "cctv" && !isSessionWsConnectable(parsed.websocketUrl, parsed.token)) {
    return null;
  }
  return parsed as DetachedHandoff;
}

export function readDetachedHandoff(sessionId: string): DetachedHandoff | null {
  return readBrowserLocalSetting<DetachedHandoff | null>(
    `${DETACHED_HANDOFF_PREFIX}${sessionId}`,
    null,
    normalizeDetachedHandoff,
  );
}

export function writeDetachedHandoff(sessionId: string, handoff: DetachedHandoff): void {
  writeBrowserLocalSetting<DetachedHandoff | null>(
    `${DETACHED_HANDOFF_PREFIX}${sessionId}`,
    handoff,
    normalizeDetachedHandoff,
  );
}

export function consumeInjectedDetachedHandoff(sessionId: string): DetachedHandoff | null {
  if (typeof window === "undefined") return null;
  const injected = window.__PIPING_DETACHED_HANDOFF__;
  try {
    delete window.__PIPING_DETACHED_HANDOFF__;
  } catch {
    window.__PIPING_DETACHED_HANDOFF__ = undefined;
  }
  const handoff = normalizeDetachedHandoff(injected);
  if (handoff?.sessionId !== sessionId) return null;
  writeDetachedHandoff(sessionId, handoff);
  return handoff;
}

export function clearDetachedHandoff(sessionId: string): void {
  removeBrowserLocalSetting(`${DETACHED_HANDOFF_PREFIX}${sessionId}`);
}

/**
 * Consume the multi-session group handoff payload the parent window injects
 * right after `window.open`. Returns the array of valid handoffs, dropping any
 * malformed entries. The matching per-session `localStorage` handoffs are
 * written so a hard reload of the group window can still restore each session.
 */
export function consumeInjectedDetachedGroupHandoff(): DetachedHandoff[] | null {
  if (typeof window === "undefined") return null;
  const injected = window.__PIPING_DETACHED_GROUP_HANDOFF__;
  try {
    delete window.__PIPING_DETACHED_GROUP_HANDOFF__;
  } catch {
    window.__PIPING_DETACHED_GROUP_HANDOFF__ = undefined;
  }
  if (!Array.isArray(injected)) return null;
  const handoffs: DetachedHandoff[] = [];
  for (const entry of injected) {
    const normalized = normalizeDetachedHandoff(entry);
    if (!normalized) continue;
    writeDetachedHandoff(normalized.sessionId, normalized);
    handoffs.push(normalized);
  }
  return handoffs.length > 0 ? handoffs : null;
}
