import {
  readBrowserLocalSetting,
  readRetiredLocalJson,
  retireLocalKey,
  writeBrowserLocalSetting,
} from "@/shared/settings";

export type FileBrowserPlacement = "bottom" | "right";

export const SSH_FILE_BROWSER_PREFERENCE_KEY = "connections.sshFileBrowser";

const DEFAULT_FILE_BROWSER_SIZES: Record<FileBrowserPlacement, number> = {
  bottom: 112,
  right: 408,
};

export const SSH_FILE_BROWSER_MIN_SIZES: Record<FileBrowserPlacement, number> = {
  bottom: 96,
  right: 280,
};

const FILE_BROWSER_MAX_VIEWPORT_RATIO: Record<FileBrowserPlacement, number> = {
  bottom: 0.58,
  right: 0.56,
};

export interface PersistedSshFileBrowserState {
  placement: FileBrowserPlacement;
  sizes: Record<FileBrowserPlacement, number>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isFileBrowserPlacement(value: unknown): value is FileBrowserPlacement {
  return value === "bottom" || value === "right";
}

export function getSshFileBrowserMaxSize(
  placement: FileBrowserPlacement,
  availableSize?: number,
): number {
  const viewportSize = typeof window === "undefined"
    ? Number.POSITIVE_INFINITY
    : placement === "bottom"
      ? window.innerHeight
      : window.innerWidth;
  const viewportMax = Math.floor(viewportSize * FILE_BROWSER_MAX_VIEWPORT_RATIO[placement]);
  const boundedAvailable = typeof availableSize === "number" && Number.isFinite(availableSize)
    ? Math.max(SSH_FILE_BROWSER_MIN_SIZES[placement], availableSize)
    : Number.POSITIVE_INFINITY;
  return Math.max(
    SSH_FILE_BROWSER_MIN_SIZES[placement],
    Math.min(viewportMax, boundedAvailable),
  );
}

export function clampSshFileBrowserSize(
  placement: FileBrowserPlacement,
  size: unknown,
  availableSize?: number,
): number {
  const fallback = DEFAULT_FILE_BROWSER_SIZES[placement];
  const numeric = typeof size === "number" && Number.isFinite(size) ? size : fallback;
  return Math.round(
    Math.max(
      SSH_FILE_BROWSER_MIN_SIZES[placement],
      Math.min(getSshFileBrowserMaxSize(placement, availableSize), numeric),
    ),
  );
}

export function defaultSshFileBrowserState(): PersistedSshFileBrowserState {
  return {
    placement: "right",
    sizes: { ...DEFAULT_FILE_BROWSER_SIZES },
  };
}

export function normalizeSshFileBrowserState(
  value: unknown,
): PersistedSshFileBrowserState {
  const defaults = defaultSshFileBrowserState();
  if (!isRecord(value)) return defaults;

  const placement = isFileBrowserPlacement(value.placement) ? value.placement : defaults.placement;
  const rawSizes = isRecord(value.sizes) ? value.sizes : {};

  return {
    placement,
    sizes: {
      bottom: clampSshFileBrowserSize("bottom", rawSizes.bottom),
      right: clampSshFileBrowserSize("right", rawSizes.right),
    },
  };
}

export function loadSshFileBrowserState(): PersistedSshFileBrowserState {
  const current = readBrowserLocalSetting(
    SSH_FILE_BROWSER_PREFERENCE_KEY,
    defaultSshFileBrowserState(),
    normalizeSshFileBrowserState,
  );
  const retired = readRetiredLocalJson(
    SSH_FILE_BROWSER_PREFERENCE_KEY,
    normalizeSshFileBrowserState,
  );
  return retired ?? current;
}

export function saveSshFileBrowserState(state: PersistedSshFileBrowserState): void {
  writeBrowserLocalSetting(
    SSH_FILE_BROWSER_PREFERENCE_KEY,
    state,
    normalizeSshFileBrowserState,
  );
  retireLocalKey(SSH_FILE_BROWSER_PREFERENCE_KEY);
}
