import { readBrowserLocalSetting, writeBrowserLocalSetting } from "@/shared/settings";

/**
 * Docked session inspector sizing on the active-connections page. Width and
 * the observation preview height are user-resizable in a fixed band and
 * persisted across sessions, like the connections panel.
 */
export const SESSION_INSPECTOR_PREFERENCE_KEY = "connections.sessions.inspector";

export const SESSION_INSPECTOR_MIN_WIDTH = 320;
export const SESSION_INSPECTOR_MAX_WIDTH = 720;
export const SESSION_INSPECTOR_DEFAULT_WIDTH = 380;

export const OBSERVATION_PREVIEW_MIN_HEIGHT = 200;
export const OBSERVATION_PREVIEW_DEFAULT_HEIGHT = 360;

const INSPECTOR_MAX_VIEWPORT_RATIO = 0.55;
const PREVIEW_MAX_VIEWPORT_RATIO = 0.7;

export interface SessionInspectorLayout {
  width: number;
  previewHeight: number;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

export function sessionInspectorMaxWidth(): number {
  if (typeof window === "undefined") return SESSION_INSPECTOR_MAX_WIDTH;
  return Math.max(
    SESSION_INSPECTOR_MIN_WIDTH,
    Math.min(
      SESSION_INSPECTOR_MAX_WIDTH,
      Math.round(window.innerWidth * INSPECTOR_MAX_VIEWPORT_RATIO),
    ),
  );
}

export function observationPreviewMaxHeight(): number {
  if (typeof window === "undefined") return OBSERVATION_PREVIEW_DEFAULT_HEIGHT * 2;
  return Math.max(
    OBSERVATION_PREVIEW_MIN_HEIGHT,
    Math.round(window.innerHeight * PREVIEW_MAX_VIEWPORT_RATIO),
  );
}

export function clampSessionInspectorWidth(
  width: unknown,
  maxWidth: number = sessionInspectorMaxWidth(),
): number {
  const numeric =
    typeof width === "number" && Number.isFinite(width)
      ? width
      : SESSION_INSPECTOR_DEFAULT_WIDTH;
  return Math.round(
    Math.max(SESSION_INSPECTOR_MIN_WIDTH, Math.min(maxWidth, numeric)),
  );
}

export function clampObservationPreviewHeight(height: unknown): number {
  const numeric =
    typeof height === "number" && Number.isFinite(height)
      ? height
      : OBSERVATION_PREVIEW_DEFAULT_HEIGHT;
  return Math.round(
    Math.max(
      OBSERVATION_PREVIEW_MIN_HEIGHT,
      Math.min(observationPreviewMaxHeight(), numeric),
    ),
  );
}

export function defaultSessionInspectorLayout(): SessionInspectorLayout {
  return {
    width: clampSessionInspectorWidth(SESSION_INSPECTOR_DEFAULT_WIDTH),
    previewHeight: clampObservationPreviewHeight(OBSERVATION_PREVIEW_DEFAULT_HEIGHT),
  };
}

export function normalizeSessionInspectorLayout(value: unknown): SessionInspectorLayout {
  const defaults = defaultSessionInspectorLayout();
  if (!isRecord(value)) return defaults;
  return {
    width: clampSessionInspectorWidth(value.width),
    previewHeight: clampObservationPreviewHeight(value.previewHeight),
  };
}

export function loadSessionInspectorLayout(): SessionInspectorLayout {
  return readBrowserLocalSetting(
    SESSION_INSPECTOR_PREFERENCE_KEY,
    defaultSessionInspectorLayout(),
    normalizeSessionInspectorLayout,
  );
}

export function saveSessionInspectorLayout(layout: SessionInspectorLayout): void {
  writeBrowserLocalSetting(
    SESSION_INSPECTOR_PREFERENCE_KEY,
    layout,
    normalizeSessionInspectorLayout,
  );
}
