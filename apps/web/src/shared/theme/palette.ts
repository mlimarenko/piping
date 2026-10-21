/**
 * White-label palette engine.
 *
 * Given a single brand base color, derive the full set of *accent-family*
 * design tokens for BOTH the dark and light themes, and apply them at runtime
 * by injecting CSS custom-property overrides into the document head.
 *
 * Scope of what we recolor: the primary actions, tinted selection surfaces,
 * their foregrounds, focus rings and sidebar selection.
 * Neutral surfaces/text and the semantic status colors (ok/warn/err/info) are
 * intentionally left untouched: a brand color must not turn "error" red into
 * the brand hue. The base design tokens live in `src/index.css` as HSL-channel
 * triplets consumed via `hsl(var(--token))`, so an override is just a new
 * triplet string under the right theme selector.
 */

export interface Hsl {
  /** Hue 0–360. */
  h: number;
  /** Saturation 0–100. */
  s: number;
  /** Lightness 0–100. */
  l: number;
}

export interface PalettePreset {
  id: string;
  /** i18n-independent display label (English); UI may localize separately. */
  label: string;
  base: Hsl;
}

/** Brand presets leave neutral surfaces and semantic statuses unchanged. */
export const PALETTE_PRESETS: readonly PalettePreset[] = [
  { id: "amber", label: "Amber", base: { h: 42, s: 95, l: 52 } },
  { id: "teal", label: "Teal", base: { h: 174, s: 78, l: 25 } },
  { id: "blue", label: "Blue", base: { h: 211, s: 90, l: 54 } },
  { id: "violet", label: "Violet", base: { h: 262, s: 83, l: 62 } },
  { id: "emerald", label: "Emerald", base: { h: 152, s: 64, l: 44 } },
  { id: "rose", label: "Rose", base: { h: 347, s: 77, l: 55 } },
  { id: "cyan", label: "Cyan", base: { h: 190, s: 85, l: 47 } },
] as const;

/** Initial color-picker and stock favicon base; stock UI tokens belong to index.css. */
export const DEFAULT_BASE: Hsl = PALETTE_PRESETS[0]!.base;

const clamp = (n: number, lo: number, hi: number): number => Math.min(hi, Math.max(lo, n));
const round = (n: number): number => Math.round(n);

/** Format an {h,s,l} as the `"H S% L%"` channel triplet `hsl(var(--x))` expects. */
const triplet = (h: number, s: number, l: number): string =>
  `${round(((h % 360) + 360) % 360)} ${round(clamp(s, 0, 100))}% ${round(clamp(l, 0, 100))}%`;

function luminance(color: Hsl): number {
  const hex = hslToHex(color);
  const channels = [1, 3, 5].map((offset) => {
    const value = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255;
    return value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return channels[0]! * 0.2126 + channels[1]! * 0.7152 + channels[2]! * 0.0722;
}

function contrast(first: Hsl, second: Hsl): number {
  const a = luminance(first);
  const b = luminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function readablePrimary(base: Hsl, surfaces: Hsl[], direction: -1 | 1): Hsl {
  const color = { ...base, l: round(base.l) };
  while (color.l > 0 && color.l < 100 && surfaces.some(surface => contrast(color, surface) < 4.5)) color.l += direction;
  return color;
}

export interface DerivedPalette {
  dark: Record<string, string>;
  light: Record<string, string>;
}

function themePalette(base: Hsl, dark: boolean): Record<string, string> {
  const h = ((base.h % 360) + 360) % 360;
  const s = clamp(base.s, 0, 100);
  const accent = { h, s: Math.min(s, dark ? 65 : 60), l: dark ? 15 : 93 };
  const accentHover = { ...accent, l: dark ? 17 : 89 };
  const accentActive = { ...accent, l: dark ? 20 : 85 };
  const sidebarAccent = { ...accent, l: dark ? 16 : 88 };
  const ink = { h: 218, s: 28, l: 9 };
  const white = { h: 0, s: 0, l: 100 };
  const primary = dark ? readablePrimary(
    { h, s, l: clamp(base.l, 52, 60) },
    [accent, accentHover, accentActive, sidebarAccent],
    1,
  ) : ink;
  const candidateFill = dark ? primary : { h, s, l: clamp(base.l, 18, 60) };
  const foreground = contrast(candidateFill, ink) > contrast(candidateFill, white) ? ink : white;
  const fill = dark ? primary : readablePrimary(candidateFill, [foreground], foreground === ink ? 1 : -1);
  const value = (color: Hsl) => triplet(color.h, color.s, color.l);
  return {
    "--primary": value(primary),
    "--primary-fill": value(fill),
    "--primary-foreground": value(foreground),
    "--accent": value(accent),
    "--accent-foreground": value(primary),
    "--accent-hover": value(accentHover),
    "--accent-active": value(accentActive),
    "--accent-dim": value(fill) + (dark ? " / 0.12" : " / 0.08"),
    "--ring": value(primary),
    "--sidebar-primary": value(fill),
    "--sidebar-primary-foreground": value(foreground),
    "--sidebar-accent": value(sidebarAccent),
    "--sidebar-accent-foreground": value(primary),
  };
}

/** Keep brand actions distinct from the quiet selection backgrounds. */
export function derivePalette(base: Hsl): DerivedPalette {
  return { dark: themePalette(base, true), light: themePalette(base, false) };
}

const STYLE_ELEMENT_ID = "wl-palette-overrides";

const renderVars = (vars: Record<string, string>): string =>
  Object.entries(vars)
    .map(([k, v]) => `  ${k}: ${v};`)
    .join("\n");

/** Match index.css: light tokens on the root, dark tokens only with .dark. */
export function paletteToCss(palette: DerivedPalette): string {
  return [
    ":root {\n" + renderVars(palette.light) + "\n}",
    ":root.dark {\n" + renderVars(palette.dark) + "\n}",
  ].join("\n");
}

/**
 * Apply a derived palette by injecting (or updating) a single `<style>` element
 * in the document head. Idempotent — call again to replace. Pass `null` to
 * remove the override and fall back to the stock `index.css` tokens.
 */
export function applyPalette(palette: DerivedPalette | null): void {
  if (typeof document === "undefined") return;
  const existing = document.getElementById(STYLE_ELEMENT_ID);
  if (!palette) {
    existing?.remove();
    return;
  }
  const css = paletteToCss(palette);
  if (existing) {
    existing.textContent = css;
    return;
  }
  const el = document.createElement("style");
  el.id = STYLE_ELEMENT_ID;
  el.textContent = css;
  document.head.appendChild(el);
}

// ── hex ⇄ hsl helpers for the color picker ───────────────────────────────────

/** Parse `#rgb`/`#rrggbb` to HSL. Returns null on malformed input. */
export function hexToHsl(hex: string): Hsl | null {
  const m = /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i.exec(hex.trim());
  if (!m?.[1]) return null;
  let raw = m[1];
  if (raw.length === 3) raw = raw.split("").map((c) => c + c).join("");
  const r = Number.parseInt(raw.slice(0, 2), 16) / 255;
  const g = Number.parseInt(raw.slice(2, 4), 16) / 255;
  const b = Number.parseInt(raw.slice(4, 6), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const d = max - min;
  let h = 0;
  if (d !== 0) {
    if (max === r) h = ((g - b) / d) % 6;
    else if (max === g) h = (b - r) / d + 2;
    else h = (r - g) / d + 4;
    h *= 60;
    if (h < 0) h += 360;
  }
  const l = (max + min) / 2;
  const s = d === 0 ? 0 : d / (1 - Math.abs(2 * l - 1));
  return { h: round(h), s: round(s * 100), l: round(l * 100) };
}

/** Convert HSL to `#rrggbb` (for seeding the native color input). */
export function hslToHex({ h, s, l }: Hsl): string {
  const sN = clamp(s, 0, 100) / 100;
  const lN = clamp(l, 0, 100) / 100;
  const c = (1 - Math.abs(2 * lN - 1)) * sN;
  const hp = (((h % 360) + 360) % 360) / 60;
  const x = c * (1 - Math.abs((hp % 2) - 1));
  let r: number;
  let g: number;
  let b: number;
  if (hp < 1) [r, g, b] = [c, x, 0];
  else if (hp < 2) [r, g, b] = [x, c, 0];
  else if (hp < 3) [r, g, b] = [0, c, x];
  else if (hp < 4) [r, g, b] = [0, x, c];
  else if (hp < 5) [r, g, b] = [x, 0, c];
  else [r, g, b] = [c, 0, x];
  const m = lN - c / 2;
  const hexByte = (v: number): string =>
    round((v + m) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${hexByte(r)}${hexByte(g)}${hexByte(b)}`;
}
