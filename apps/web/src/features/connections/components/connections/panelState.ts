import type { SessionProtocol } from "@/shared/api/types";
import type { DockPosition } from "@/features/connections/components/connections/ConnectionsPanel";
import { normalizeSessionPaneLayout, type SessionPaneLayout } from "./sessionPaneLayout";

export const CONNECTIONS_PANEL_PREFERENCE_KEY = "connections.workspace.panel";

const DEFAULT_PANEL_SIZES: Record<DockPosition, number> = {
  bottom: 320,
  right: 480,
};

export const CONNECTIONS_PANEL_MIN_SIZES: Record<DockPosition, number> = {
  bottom: 140,
  right: 320,
};

const PANEL_MAX_VIEWPORT_RATIO = 0.85;

export interface PersistedPanelActiveSession {
  sessionId?: string;
  hostId?: string;
  protocol?: SessionProtocol;
}

export interface PersistedConnectionsPanelState {
  paneLayout: SessionPaneLayout | null;
  showPanel: boolean;
  expanded: boolean;
  dockPosition: DockPosition;
  sizes: Record<DockPosition, number>;
  activeSession: PersistedPanelActiveSession | null;
}

interface PanelSessionCandidate {
  id: string;
  hostId: string;
  protocol: SessionProtocol;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isDockPosition(value: unknown): value is DockPosition {
  return value === "bottom" || value === "right";
}

function isSessionProtocol(value: unknown): value is SessionProtocol {
  return value === "ssh" || value === "rdp" || value === "vnc" || value === "cctv";
}

function finitePositiveSize(size: unknown): number | null {
  return typeof size === "number" && Number.isFinite(size) && size > 0
    ? Math.floor(size)
    : null;
}

function maxPanelSize(dockPosition: DockPosition, availableSize?: number): number {
  const availableMaxSize = finitePositiveSize(availableSize);
  if (typeof window === "undefined") {
    return availableMaxSize ?? Number.POSITIVE_INFINITY;
  }
  const viewportSize = dockPosition === "bottom" ? window.innerHeight : window.innerWidth;
  const viewportMaxSize = Math.floor(viewportSize * PANEL_MAX_VIEWPORT_RATIO);
  return Math.max(
    CONNECTIONS_PANEL_MIN_SIZES[dockPosition],
    Math.min(viewportMaxSize, availableMaxSize ?? viewportMaxSize),
  );
}

export function clampConnectionsPanelSize(
  dockPosition: DockPosition,
  size: unknown,
  availableSize?: number,
): number {
  const fallback = DEFAULT_PANEL_SIZES[dockPosition];
  const numeric = typeof size === "number" && Number.isFinite(size) ? size : fallback;
  return Math.round(
    Math.max(
      CONNECTIONS_PANEL_MIN_SIZES[dockPosition],
      Math.min(maxPanelSize(dockPosition, availableSize), numeric),
    ),
  );
}

export function defaultConnectionsPanelState(isMobile: boolean): PersistedConnectionsPanelState {
  return {
    showPanel: false,
    expanded: true,
    dockPosition: isMobile ? "bottom" : "right",
    sizes: { ...DEFAULT_PANEL_SIZES },
    activeSession: null,
    paneLayout: null,
  };
}

function normalizePanelSizes(
  raw: Record<string, unknown>,
  dockPosition: DockPosition,
): Record<DockPosition, number> {
  const rawSizes = isRecord(raw.sizes) ? raw.sizes : {};
  const panelSizeBeforeDocking = raw.panelSize;
  return {
    bottom: clampConnectionsPanelSize(
      "bottom",
      rawSizes.bottom ?? (dockPosition === "bottom" ? panelSizeBeforeDocking : undefined),
    ),
    right: clampConnectionsPanelSize(
      "right",
      rawSizes.right ?? (dockPosition === "right" ? panelSizeBeforeDocking : undefined),
    ),
  };
}

function normalizeActiveSession(value: unknown): PersistedPanelActiveSession | null {
  if (!isRecord(value)) return null;

  const sessionId = typeof value.sessionId === "string" && value.sessionId.length > 0
    ? value.sessionId
    : undefined;
  const hostId = typeof value.hostId === "string" && value.hostId.length > 0
    ? value.hostId
    : undefined;
  const protocol = isSessionProtocol(value.protocol) ? value.protocol : undefined;

  if (!sessionId && (!hostId || !protocol)) return null;
  return { sessionId, hostId, protocol };
}

export function normalizeConnectionsPanelState(
  value: unknown,
  isMobile: boolean,
): PersistedConnectionsPanelState {
  const defaults = defaultConnectionsPanelState(isMobile);
  if (!isRecord(value)) return defaults;

  const dockPosition = isDockPosition(value.dockPosition)
    ? value.dockPosition
    : defaults.dockPosition;

  return {
    showPanel: typeof value.showPanel === "boolean" ? value.showPanel : defaults.showPanel,
    expanded: typeof value.expanded === "boolean" ? value.expanded : defaults.expanded,
    dockPosition,
    sizes: normalizePanelSizes(value, dockPosition),
    activeSession: normalizeActiveSession(value.activeSession),
    paneLayout: normalizeSessionPaneLayout(value.paneLayout),
  };
}

export function activePanelSessionFromLiveSession(
  session: PanelSessionCandidate,
): PersistedPanelActiveSession {
  return {
    sessionId: session.id,
    hostId: session.hostId,
    protocol: session.protocol,
  };
}

export function findPreferredPanelSession<T extends PanelSessionCandidate>(
  sessions: T[],
  activeSession: PersistedPanelActiveSession | null,
): T | null {
  if (!activeSession) return null;

  if (activeSession.sessionId) {
    const exact = sessions.find((session) => session.id === activeSession.sessionId);
    if (exact) return exact;
  }

  if (!activeSession.hostId || !activeSession.protocol) return null;
  return [...sessions]
    .reverse()
    .find(
      (session) =>
        session.hostId === activeSession.hostId &&
        session.protocol === activeSession.protocol,
    ) ?? null;
}
