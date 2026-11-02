/**
 * Apply org-wide white-label branding to the running document.
 *
 * This is the single side-effecting entry point that takes a `Branding`
 * record (from `GET /api/v1/instance/branding`) and reflects it into the live
 * page: the favicon `<link>` and the accent palette. The browser-tab title is
 * NOT set here — it is dynamic and owned by `useDocumentTitle` at the route
 * level (the instance name still flows into it via `useBranding().appName`).
 *
 * Palette derivation/injection is delegated to `palette.ts` (the canonical
 * engine) — this module only owns the favicon plumbing and the interpretation
 * of the `theme` JSON shape.
 */
import type { Branding, BrandingTheme } from "@/shared/api/branding-api";
import { applyPalette, derivePalette, DEFAULT_BASE, hslToHex, type Hsl } from "@/shared/theme/palette";

function defaultFaviconDataUrl(base: Hsl): string {
  const fill = hslToHex(base);
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">` +
    `<defs>` +
    `<mask id="a"><rect width="32" height="32" fill="black"/>` +
    `<circle cx="12" cy="20" r="12" fill="white"/><circle cx="20" cy="12" r="9" fill="black"/></mask>` +
    `<mask id="b"><rect width="32" height="32" fill="black"/>` +
    `<circle cx="20" cy="12" r="9" fill="white"/><circle cx="12" cy="20" r="12" fill="black"/></mask>` +
    `</defs>` +
    `<circle cx="12" cy="20" r="12" fill="${fill}" mask="url(#a)"/>` +
    `<circle cx="20" cy="12" r="9" fill="${fill}" opacity=".9" mask="url(#b)"/>` +
    `</svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}

/**
 * Parse the opaque `theme` JSON into our discriminated shape. Null-safe and
 * tolerant of malformed/legacy values — returns `null` when nothing usable is
 * present so callers fall back to the stock palette.
 */
export function parseBrandingTheme(theme: unknown): BrandingTheme | null {
  if (!theme || typeof theme !== "object") return null;
  const t = theme as Record<string, unknown>;
  const mode = t.mode;
  if (mode !== "preset" && mode !== "custom" && mode !== "default") return null;
  const result: BrandingTheme = { mode };
  if (typeof t.presetId === "string") result.presetId = t.presetId;
  const base = t.base;
  if (base && typeof base === "object") {
    const b = base as Record<string, unknown>;
    if (
      typeof b.h === "number" &&
      typeof b.s === "number" &&
      typeof b.l === "number"
    ) {
      result.base = { h: b.h, s: b.s, l: b.l };
    }
  }
  return result;
}

/** Extract the brand base color from a parsed theme, if one applies. */
export function brandingBase(theme: unknown): Hsl | null {
  const parsed = parseBrandingTheme(theme);
  if (!parsed || parsed.mode === "default") return null;
  return parsed.base ?? null;
}

/** Set/replace the favicon `<link rel="icon">` href. */
function applyFavicon(href: string): void {
  if (typeof document === "undefined") return;
  let link = document.querySelector<HTMLLinkElement>('link[rel="icon"]');
  if (!link) {
    link = document.createElement("link");
    link.rel = "icon";
    document.head.appendChild(link);
  }
  // Drop the SVG type hint when swapping to a data URL of unknown format.
  link.removeAttribute("type");
  link.href = href;
}

/**
 * Reflect a branding record into the live document. Null-safe and SSR-safe.
 *
 * - favicon ← `favicon_data_url`, or generated mark in the brand base color
 * - palette ← derived from `theme.base`, or stock when mode is "default"
 */
export function applyBranding(branding: Branding | null | undefined): void {
  if (typeof document === "undefined") return;

  // NOTE: `document.title` is intentionally NOT set here. The tab title is
  // dynamic (`<instance> · <page>`, or `<host> · <protocol>` for a detached
  // session) and owned solely by `useDocumentTitle` at the route level. The
  // instance name still drives the title — it flows in as the leading segment
  // via `useBranding().appName`. Setting it here would clobber the route title
  // on every branding load (BrandingProvider is a parent of the routes), most
  // visibly collapsing a freshly opened detached-session tab to the bare name.

  const base = brandingBase(branding?.theme);
  // A color-specific data URL avoids the browser's cache of /favicon.svg.
  // Uploaded artwork takes precedence and is not recolored.
  applyFavicon(branding?.favicon_data_url ?? defaultFaviconDataUrl(base ?? DEFAULT_BASE));
  applyPalette(base ? derivePalette(base) : null);
}
