import createClient, { type Middleware } from "openapi-fetch";
import type { paths } from "./schema.gen";

// ── Config ───────────────────────────────────────────────

/**
 * Base URL of the backend.
 *
 * - In dev, Vite proxies `/api/v1/*` to the backend (see `vite.config.ts`), so
 *   leave this empty and use relative paths.
 * - In prod, the web bundle is served same-origin behind nginx, so this also
 *   stays empty.
 * - Only set `VITE_API_BASE_URL` when you genuinely point at a different
 *   origin during dev (requires backend `cors_allowed_origins` + `credentials`).
 */
export const API_BASE_URL: string =
  (import.meta as { env?: { VITE_API_BASE_URL?: string } }).env?.VITE_API_BASE_URL?.replace(
    /\/$/,
    "",
  ) ?? "";

const expiryListeners = new Set<() => void>();
let refreshInFlight: Promise<boolean> | undefined;
let probeInFlight: Promise<Response> | undefined;
let sessionGeneration = 0;
let expiryNotified = false;

export function subscribeSessionExpired(listener: () => void): () => void {
  expiryListeners.add(listener);
  return () => { expiryListeners.delete(listener); };
}

function sessionExpired(): void {
  if (expiryNotified) return;
  expiryNotified = true;
  for (const listener of expiryListeners) listener();
}

function sessionEndpoint(path: string, origin = window.location.origin): string {
  return new URL(`${API_BASE_URL}${path}`, origin).href;
}

/** The server only extends active sessions; expired/revoked sessions stay denied. */
export function refreshSession(origin?: string): Promise<boolean> {
  refreshInFlight ??= (async () => {
    const response = await fetch(new Request(sessionEndpoint("/api/v1/auth/session-refresh", origin), {
      method: "POST", credentials: "include", signal: AbortSignal.timeout(15_000),
    }));
    if (response.status === 401) {
      sessionExpired();
      return false;
    }
    if (!response.ok) throw await apiErrorFromResponse(response);
    expiryNotified = false;
    sessionGeneration += 1;
    return true;
  })().finally(() => { refreshInFlight = undefined; });
  return refreshInFlight;
}

/** One recovery path for typed JSON, binary downloads and multipart uploads. */
export async function fetchWithSession(input: RequestInfo | URL, init?: RequestInit): Promise<Response> {
  const url = input instanceof Request ? input.url : new URL(String(input), window.location.href).href;
  const requestInput = input instanceof Request ? input : url;
  const request = input instanceof Request && !init ? input : new Request(requestInput, { credentials: "include", ...init });
  return sendWithSession(request, (attempt) => fetch(attempt));
}

/** Shared auth decision for fetch and progress-reporting XHR uploads. */
export async function sendWithSession(request: Request, send: (attempt: Request) => Promise<Response>): Promise<Response> {
  const path = new URL(request.url).pathname;
  const apiOrigin = new URL(API_BASE_URL || window.location.origin, window.location.origin).origin;
  const protectedRequest = new URL(request.url).origin === apiOrigin && path.startsWith("/api/v1/")
    && !["/api/v1/auth/login", "/api/v1/auth/logout", "/api/v1/auth/providers", "/api/v1/auth/session-refresh"].includes(path)
    && !request.headers.has("Authorization");
  // Clone before the first send consumes a POST/PUT/multipart body.
  const retry = protectedRequest ? request.clone() : undefined;
  const generation = sessionGeneration;
  let response = await send(request);
  if (response.ok && (path === "/api/v1/auth/login" || path === "/api/v1/auth/me")) expiryNotified = false;
  if (!retry || (response.status !== 401 && response.status !== 403)) return response;

  if (response.status === 403) {
    // Some route guards answer anonymous requests with 403. Check authentication
    // independently: a valid user's real RBAC denial must never trigger logout.
    probeInFlight ??= fetch(new Request(sessionEndpoint("/api/v1/auth/me", new URL(request.url).origin), {
      credentials: "include", signal: AbortSignal.timeout(15_000),
    })).finally(() => { probeInFlight = undefined; });
    const probe = await probeInFlight;
    if (probe.status !== 401 && generation === sessionGeneration) return response;
  }
  if (generation === sessionGeneration && !await refreshSession(new URL(request.url).origin)) return response;
  response = await send(retry);
  if (response.status === 401) sessionExpired();
  return response;
}

// ── Error type ───────────────────────────────────────────

interface ApiErrorInit {
  status: number;
  message: string;
  code?: string;
  reasonCode?: string;
  traceId?: string;
  fields?: Record<string, string>;
  details?: unknown;
}

export class ApiError extends Error {
  readonly status: number;
  /** Canonical backend error code, e.g. "VALIDATION_ERROR". */
  readonly code?: string;
  /** Machine-readable reason, populated from the canonical envelope. */
  readonly reasonCode?: string;
  /** Correlation id for logs/traces. */
  readonly traceId?: string;
  /** Per-field validation errors keyed by form field name. */
  readonly fields?: Record<string, string>;
  /** Structured backend details for richer recovery flows. */
  readonly details?: unknown;

  constructor(init: ApiErrorInit) {
    super(init.message);
    this.name = "ApiError";
    this.status = init.status;
    this.code = init.code;
    this.reasonCode = init.reasonCode;
    this.traceId = init.traceId;
    this.fields = init.fields;
    this.details = init.details;
  }
}

// ── Typed openapi-fetch client ───────────────────────────

function makeRequestId(): string {
  if (typeof crypto !== "undefined" && typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }
  return `req-${Date.now().toString(16)}-${Math.random().toString(16).slice(2, 10)}`;
}

const requestIdMiddleware: Middleware = {
  onRequest({ request }) {
    if (!request.headers.has("X-Request-Id")) {
      request.headers.set("X-Request-Id", makeRequestId());
    }
    return request;
  },
  async onResponse({ response }) {
    const requestId = response.headers.get("X-Request-Id");
    if (requestId) {
      const obs = await import("@/shared/lib/observability");
      obs.tagRequestId(requestId);
    }
    return response;
  },
};

export async function apiErrorFromResponse(response: Response): Promise<ApiError> {
  let body: Record<string, unknown> | null = null;
  try {
    body = (await response.clone().json()) as Record<string, unknown>;
  } catch {
    // Binary/plain-text failures intentionally fall back to safe generic copy.
  }
  const rawCode = body?.code;
  const rawReason = body?.reason_code;
  const rawTrace = body?.trace_id;
  const rawFields = body?.fields;
  return new ApiError({
    status: response.status,
    message: typeof body?.message === "string" ? body.message : response.statusText || "Request failed",
    code: typeof rawCode === "string" ? rawCode : undefined,
    reasonCode: typeof rawReason === "string" ? rawReason : undefined,
    traceId: typeof rawTrace === "string" ? rawTrace : undefined,
    fields:
      rawFields && typeof rawFields === "object" && !Array.isArray(rawFields)
        ? (rawFields as Record<string, string>)
        : undefined,
    details: body?.details,
  });
}

const errorMiddleware: Middleware = {
  async onResponse({ response }) {
    if (response.ok) return response;
    throw await apiErrorFromResponse(response);
  },
};

export const apiClient = createClient<paths>({
  baseUrl: API_BASE_URL,
  credentials: "include",
  fetch: fetchWithSession,
});

apiClient.use(requestIdMiddleware, errorMiddleware);

/**
 * Unwrap an openapi-fetch result. The error middleware above re-throws on
 * non-2xx, so in practice the `error` branch is unreachable; this helper
 * exists to satisfy `@typescript-eslint/only-throw-error` and to assert
 * that 2xx responses carry a body.
 */
export function unwrap<T>(result: { data?: T; error?: unknown }): T {
  if (result.error !== undefined) {
    if (result.error instanceof Error) throw result.error;
    throw new ApiError({
      status: 0,
      message: typeof result.error === "string" ? result.error : "Request failed",
    });
  }
  if (result.data === undefined) {
    throw new ApiError({ status: 0, message: "Empty response body" });
  }
  return result.data;
}

export type { paths } from "./schema.gen";
export type { components } from "./schema.gen";
