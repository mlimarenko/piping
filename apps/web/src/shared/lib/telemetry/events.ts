/**
 * Fixed product-event vocabulary shared by call sites and the relay.
 *
 * A new event is a code change here and in the backend allow-list, never a
 * free-form string. Funnels are derived from the event, not supplied by the
 * browser.
 */
export const PRODUCT_EVENTS = [
  "app_opened",
  "connection_session_started",
  "playbook_run_started",
] as const;

export type ProductEvent = (typeof PRODUCT_EVENTS)[number];

export type WebVitalMetric = "lcp" | "cls" | "inp" | "fcp" | "ttfb";

/**
 * Route segments the relay accepts. Anything else leaves the browser as
 * `other`, so an identifier, a slug or an attacker-chosen path never becomes a
 * report value. Mirrors the closed list in the backend `metrics/web.rs`; a new
 * route is a paired code change, not a free-form string.
 */
export const ROUTE_SEGMENTS = [
  "access-control",
  "admin",
  "api-docs",
  "assistant",
  "audit",
  "auth",
  "automation-secrets",
  "backup",
  "config-contexts",
  "connections",
  "custom-fields",
  "device-dashboard",
  "health",
  "inventory",
  "license",
  "licensing",
  "mcp",
  "metrics",
  "monitoring",
  "observability",
  "playbooks",
  "providers",
  "proxy-nodes",
  "secrets",
  "settings",
  "sources",
  "sync-jobs",
  "user-settings",
  "webhooks",
  "instance-branding",
] as const;

const OTHER_ROUTE = "other";
const MAX_STACK_CHARS = 4_000;

/** Collapse a path to one allow-listed segment; query and ids never survive. */
export function routeSegment(raw: string): string {
  const [path = ""] = raw.split(/[?#]/, 1);
  const [segment = ""] = path
    .replace(/^\/api\/v1\//, "")
    .replace(/^\/+/, "")
    .split("/");
  return (ROUTE_SEGMENTS as readonly string[]).includes(segment)
    ? segment
    : OTHER_ROUTE;
}

/** Cap a stack before it leaves the tab; the relay re-sanitises every frame. */
export function boundedStack(stack: string | undefined): string | undefined {
  if (stack === undefined) return undefined;
  const trimmed = stack.trim();
  if (trimmed === "") return undefined;
  return trimmed.length > MAX_STACK_CHARS
    ? trimmed.slice(0, MAX_STACK_CHARS)
    : trimmed;
}

/**
 * One browser report. The relay validates every field again; this shape only
 * keeps call sites honest. No message, user id, form value or raw URL exists in
 * the contract.
 */
export type TelemetryEvent =
  | { type: "vital"; metric: WebVitalMetric; value: number }
  | {
      type: "js_error";
      source: "uncaught" | "unhandled_rejection";
      route: string;
      stack?: string;
    }
  | { type: "api_failure"; route: string; status: number }
  | { type: "product_event"; event: ProductEvent };
