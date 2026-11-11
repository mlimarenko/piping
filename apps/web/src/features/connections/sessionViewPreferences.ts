/**
 * Per-host display and image-quality preferences for graphical sessions.
 *
 * These are operator-local browser choices, not server state: the connection
 * config owns the quality default, and an absent (`connection`) quality leaves
 * that default untouched. A preset selected here overrides the connection's
 * colour depth / lossless setting on the next launch.
 */

export type SessionDisplayMode = "fit" | "actual";
export type SessionQuality = "connection" | "sharp" | "balanced" | "data_saver";

export interface SessionViewSettings {
  displayMode: SessionDisplayMode;
  quality: SessionQuality;
}

export const DEFAULT_SESSION_VIEW: SessionViewSettings = {
  displayMode: "fit",
  quality: "connection",
};

/** Hard ceiling for the data-saver preset, in remote pixels. */
export const DATA_SAVER_MAX_RESOLUTION: { width: number; height: number } = {
  width: 1280,
  height: 720,
};

const STORAGE_PREFIX = "piping:session-view:v1:";
const cache = new Map<string, SessionViewSettings>();
const listeners = new Map<string, Set<() => void>>();

function storageKey(hostId: string, protocol: string): string {
  return `${STORAGE_PREFIX}${protocol}:${hostId}`;
}

function isDisplayMode(value: unknown): value is SessionDisplayMode {
  return value === "fit" || value === "actual";
}

function isQuality(value: unknown): value is SessionQuality {
  return value === "connection" || value === "sharp" || value === "balanced" || value === "data_saver";
}

function normalize(raw: unknown): SessionViewSettings {
  if (typeof raw !== "object" || raw === null) return DEFAULT_SESSION_VIEW;
  const candidate = raw as Partial<SessionViewSettings>;
  return {
    displayMode: isDisplayMode(candidate.displayMode) ? candidate.displayMode : DEFAULT_SESSION_VIEW.displayMode,
    quality: isQuality(candidate.quality) ? candidate.quality : DEFAULT_SESSION_VIEW.quality,
  };
}

function readStorage(hostId: string, protocol: string): SessionViewSettings {
  if (!hostId || typeof window === "undefined") return DEFAULT_SESSION_VIEW;
  try {
    const raw = window.localStorage.getItem(storageKey(hostId, protocol));
    return raw ? normalize(JSON.parse(raw)) : DEFAULT_SESSION_VIEW;
  } catch {
    return DEFAULT_SESSION_VIEW;
  }
}

/**
 * Read the current preference for one host and protocol. The returned object is
 * stable until the preference changes, so it is safe as a `useSyncExternalStore`
 * snapshot.
 */
export function loadSessionViewSettings(
  hostId: string,
  protocol: string,
): SessionViewSettings {
  if (!hostId) return DEFAULT_SESSION_VIEW;
  const key = storageKey(hostId, protocol);
  const cached = cache.get(key);
  if (cached) return cached;
  const value = readStorage(hostId, protocol);
  cache.set(key, value);
  return value;
}

export function saveSessionViewSettings(
  hostId: string,
  protocol: string,
  patch: Partial<SessionViewSettings>,
): SessionViewSettings {
  if (!hostId) return DEFAULT_SESSION_VIEW;
  const next = normalize({ ...loadSessionViewSettings(hostId, protocol), ...patch });
  const key = storageKey(hostId, protocol);
  cache.set(key, next);
  try {
    window.localStorage.setItem(key, JSON.stringify(next));
  } catch {
    // Quota/denied storage is non-fatal: the session still uses the value.
  }
  for (const listener of listeners.get(key) ?? []) listener();
  return next;
}

export function subscribeSessionViewSettings(
  hostId: string,
  protocol: string,
  listener: () => void,
): () => void {
  if (!hostId) return () => undefined;
  const key = storageKey(hostId, protocol);
  const bucket = listeners.get(key) ?? new Set<() => void>();
  bucket.add(listener);
  listeners.set(key, bucket);
  return () => {
    bucket.delete(listener);
    if (bucket.size === 0) listeners.delete(key);
  };
}

/**
 * Launch params for a local quality override. `undefined` means "use the
 * connection's own options". Colour depth and lossless are guacd connection
 * parameters, so the override takes effect on the next session launch.
 */
export function sessionViewLaunchOptions(
  protocol: string,
  quality: SessionQuality,
): Record<string, number | boolean> | undefined {
  if (protocol !== "rdp" && protocol !== "vnc") return undefined;
  switch (quality) {
    case "sharp":
      return { color_depth: 24, force_lossless: true };
    case "balanced":
      return { color_depth: 16, force_lossless: false };
    case "data_saver":
      return { color_depth: 8, force_lossless: false };
    case "connection":
      return undefined;
  }
}

/** Remote resolution cap applied while a data-saver session is live. */
export function sessionViewMaxResolution(
  quality: SessionQuality,
): { width: number; height: number } | null {
  return quality === "data_saver" ? DATA_SAVER_MAX_RESOLUTION : null;
}

// Keep an already-open session in sync when another tab changes the preference.
if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    if (!event.key || !event.key.startsWith(STORAGE_PREFIX)) return;
    const key = event.key;
    const hostProtocol = key.slice(STORAGE_PREFIX.length).split(":");
    if (hostProtocol.length !== 2) return;
    cache.delete(key);
    for (const listener of listeners.get(key) ?? []) listener();
  });
}
