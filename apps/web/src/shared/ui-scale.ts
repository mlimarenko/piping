export const UI_SCALE_PREFERENCE_KEY = "ui.scale";
export const DEFAULT_UI_SCALE = 1;
export const MIN_UI_SCALE = 0.9;
export const MAX_UI_SCALE = 1.5;
export const UI_SCALE_STEP = 0.05;

const BASE_ROOT_FONT_SIZE_PX = 16;

export function sanitizeUiScale(value: unknown): number {
  const numeric = typeof value === "number" ? value : Number(value);
  if (!Number.isFinite(numeric)) return DEFAULT_UI_SCALE;

  const clamped = Math.min(MAX_UI_SCALE, Math.max(MIN_UI_SCALE, numeric));
  const stepped = Math.round(clamped / UI_SCALE_STEP) * UI_SCALE_STEP;
  return Number(stepped.toFixed(2));
}

export function uiScaleToPercent(scale: number): number {
  return Math.round(sanitizeUiScale(scale) * 100);
}

export function percentToUiScale(percent: number): number {
  return sanitizeUiScale(percent / 100);
}

export function applyUiScale(
  scale: unknown,
  root: HTMLElement = document.documentElement,
): void {
  const normalized = sanitizeUiScale(scale);
  root.style.fontSize = `${BASE_ROOT_FONT_SIZE_PX * normalized}px`;
  root.dataset.uiScale = String(normalized);
}

export function readAppliedUiScale(root: HTMLElement = document.documentElement): number {
  const fromDataset = sanitizeUiScale(root.dataset.uiScale);
  if (fromDataset !== DEFAULT_UI_SCALE || root.dataset.uiScale !== undefined) {
    return fromDataset;
  }

  const fontSize = Number.parseFloat(getComputedStyle(root).fontSize);
  if (!Number.isFinite(fontSize) || fontSize <= 0) return DEFAULT_UI_SCALE;
  return sanitizeUiScale(fontSize / BASE_ROOT_FONT_SIZE_PX);
}
