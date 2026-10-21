export interface DetailedMapConfig {
  styleUrl: string;
  pmtilesUrl: string;
  fallbackSvgUrl: string;
  sourceId: string;
  initialCenter: [number, number]; // allow: canonical-vocab
  initialZoom: number;
  loadTimeoutMs: number;
}

function parseCenter(value: string | undefined): [number, number] | null {
  if (!value) return null;
  const [lngRaw, latRaw] = value.split(",");
  const lng = Number(lngRaw);
  const lat = Number(latRaw);
  if (!Number.isFinite(lng) || !Number.isFinite(lat)) return null;
  return [lng, lat];
}

function parseNumber(value: string | undefined, fallback: number): number {
  if (!value) return fallback;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function envString(key: keyof ImportMetaEnv): string | undefined {
  const value: unknown = import.meta.env[key];
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

export const detailedMapConfig: DetailedMapConfig = {
  styleUrl: envString("VITE_PIPING_MAP_STYLE_URL") ?? "/map-style/style.json",
  pmtilesUrl: envString("VITE_PIPING_MAP_PMTILES_URL") ?? "/maps/world.pmtiles",
  fallbackSvgUrl: envString("VITE_PIPING_MAP_FALLBACK_SVG_URL") ?? "/fallback/simple-map.svg",
  sourceId: envString("VITE_PIPING_MAP_SOURCE_ID") ?? "basemap",
  initialCenter: parseCenter(envString("VITE_PIPING_MAP_INITIAL_CENTER")) ?? [20, 48],
  initialZoom: parseNumber(envString("VITE_PIPING_MAP_INITIAL_ZOOM"), 3),
  loadTimeoutMs: parseNumber(envString("VITE_PIPING_MAP_LOAD_TIMEOUT_MS"), 8000),
};

const PMTILES_PROTOCOL = "pmtiles://";

function browserOrigin(): string | undefined {
  return typeof window === "undefined" ? undefined : window.location.origin;
}

function hasScheme(url: string): boolean {
  return /^[a-z][a-z\d+.-]*:/i.test(url) || url.startsWith("//");
}

export function toBrowserAssetUrl(url: string, baseOrigin = browserOrigin()): string {
  if (!baseOrigin || hasScheme(url)) return url;
  return new URL(url, baseOrigin)
    .toString()
    .replace(/%7B/gi, "{")
    .replace(/%7D/gi, "}");
}

export function toPmtilesFetchUrl(url: string, baseOrigin = browserOrigin()): string {
  const rawUrl = url.startsWith(PMTILES_PROTOCOL) ? url.slice(PMTILES_PROTOCOL.length) : url;
  return toBrowserAssetUrl(rawUrl, baseOrigin);
}

export function toPmtilesProtocolUrl(url: string, baseOrigin = browserOrigin()): string {
  return `${PMTILES_PROTOCOL}${toPmtilesFetchUrl(url, baseOrigin)}`;
}
