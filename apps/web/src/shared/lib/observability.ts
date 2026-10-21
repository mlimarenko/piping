/**
 * Sentry-free observability facade.
 *
 * `@sentry/react` is ~85 KB gzipped — a fifth of the initial payload if any
 * module in the entry's static import graph pulls it in. Nothing here imports
 * it: the SDK lives behind the dynamic `import("./observability-sentry")`
 * below and lands in its own chunk fetched after the first render.
 *
 * Until that chunk resolves, errors are not lost. `initObservability()`
 * installs buffering `error`/`unhandledrejection` listeners synchronously, and
 * `captureException` from application code (per-feature `<ErrorBoundary>` in
 * `src/shared/ui-shared/ErrorBoundary.tsx`, the TanStack mutation `onError` in
 * `src/app/App.tsx`) queues into the same buffer. Once the SDK is up the buffer
 * is replayed through it and the temporary listeners are removed — Sentry
 * installs its own global handlers at that point.
 *
 * DSN comes from `VITE_SENTRY_DSN`. If absent, observability stays inert — no
 * network calls, no console spam — so dev/test runs never need a DSN.
 * Environment is `VITE_SENTRY_ENVIRONMENT` (default: `"production"` for prod
 * builds, `"development"` for dev), release tag from `VITE_RELEASE_VERSION`.
 *
 * X-Request-Id from the API client (`src/shared/api/client.ts`) is attached as
 * a `traceId` tag so Sentry events line up with backend logs (see
 * `apps/api/src/observability/`).
 */

import type * as ObservabilitySentry from "./observability-sentry";

export interface CaptureContext {
  /** Indexed string tags, e.g. `{ errorBoundary: "connections" }`. */
  tags?: Record<string, string>;
  /** Non-indexed detail, e.g. a React component stack. */
  extras?: Record<string, unknown>;
}

export interface SentryBridge {
  capture: (error: unknown, context: CaptureContext) => void;
  tagRequestId: (requestId: string) => void;
}

/**
 * `pending` — buffering; the SDK load has not finished.
 * `active`  — SDK loaded; forward straight through.
 * `off`     — no DSN, or the SDK chunk failed to load: drop everything.
 */
type State = "pending" | "active" | "off";

/**
 * Cap the buffer so an error loop during boot cannot grow it without bound.
 * An early error storm is characterised well enough by its first events.
 */
const MAX_BUFFERED_EVENTS = 50;

/** Upper bound on how long the SDK load may wait for an idle moment. */
const IDLE_TIMEOUT_MS = 2000;

interface BufferedEvent {
  error: unknown;
  context: CaptureContext;
}

let state: State = "pending";
let bridge: SentryBridge | null = null;
let buffered: BufferedEvent[] = [];
/** Errors already queued, so a window listener cannot duplicate an explicit capture. */
const seen = new WeakSet<object>();
let pendingRequestId: string | null = null;
let configuredDsn: string | undefined;

function bufferEvent(error: unknown, context: CaptureContext): void {
  if (typeof error === "object" && error !== null) {
    if (seen.has(error)) return;
    seen.add(error);
  }
  if (buffered.length >= MAX_BUFFERED_EVENTS) return;
  buffered.push({ error, context });
}

function onWindowError(event: ErrorEvent): void {
  bufferEvent(event.error ?? new Error(event.message), { tags: { earlyCapture: "error" } });
}

function onUnhandledRejection(event: PromiseRejectionEvent): void {
  bufferEvent(event.reason, { tags: { earlyCapture: "unhandledrejection" } });
}

function removeEarlyListeners(): void {
  window.removeEventListener("error", onWindowError);
  window.removeEventListener("unhandledrejection", onUnhandledRejection);
}

function shutDown(): void {
  state = "off";
  buffered = [];
  pendingRequestId = null;
  removeEarlyListeners();
}

/**
 * Report an error. Safe to call at any time: before the SDK loads the event is
 * buffered, after it loads it is forwarded, and with no DSN it is dropped.
 */
export function captureException(error: unknown, context: CaptureContext = {}): void {
  if (state === "off") return;
  if (state === "active" && bridge) {
    bridge.capture(error, context);
    return;
  }
  bufferEvent(error, context);
}

/**
 * Tag subsequent reports with a backend correlation id. Called from the API
 * client whenever a response carries `X-Request-Id`.
 */
export function tagRequestId(requestId: string): void {
  if (state === "off") return;
  if (state === "active" && bridge) {
    bridge.tagRequestId(requestId);
    return;
  }
  pendingRequestId = requestId;
}

/**
 * Run `task` once the browser is idle. The `timeout` matters: on a permanently
 * busy tab an untimed idle callback may never fire, which would mean error
 * reporting silently never starts.
 */
function whenIdle(task: () => void): void {
  const idle = (
    window as {
      requestIdleCallback?: (callback: () => void, options?: { timeout: number }) => number;
    }
  ).requestIdleCallback;
  if (typeof idle === "function") {
    idle(task, { timeout: IDLE_TIMEOUT_MS });
    return;
  }
  window.setTimeout(task, 0);
}

function readEnv(): Record<string, string | undefined> {
  return (import.meta as { env?: Record<string, string | undefined> }).env ?? {};
}

function adoptSentry(module: typeof ObservabilitySentry, dsn: string): void {
  const env = readEnv();
  bridge = module.startSentry({
    dsn,
    environment: env.VITE_SENTRY_ENVIRONMENT ?? (env.PROD ? "production" : "development"),
    release: env.VITE_RELEASE_VERSION,
    tracesSampleRate: env.PROD ? 0.1 : 1.0,
  });
  state = "active";
  removeEarlyListeners();

  if (pendingRequestId !== null) {
    bridge.tagRequestId(pendingRequestId);
    pendingRequestId = null;
  }
  const replay = buffered;
  buffered = [];
  for (const event of replay) {
    bridge.capture(event.error, event.context);
  }
}

/**
 * Install early error capture. Call this synchronously from the entry point
 * BEFORE the first render — these listeners are what make boot-time errors
 * survivable while the SDK is still absent.
 *
 * Loading the SDK is a separate, later step (`loadObservability`) so the fetch
 * cannot compete with the first paint. The DSN defaults to `VITE_SENTRY_DSN`;
 * pass one explicitly to configure reporting from somewhere other than the
 * build environment.
 */
export function initObservability(dsn: string | undefined = readEnv().VITE_SENTRY_DSN): void {
  configuredDsn = dsn;
  if (!dsn) {
    // Nothing will ever drain the buffer, so do not keep one — nor the
    // listeners that would feed it for the rest of the session.
    shutDown();
    return;
  }

  window.addEventListener("error", onWindowError);
  window.addEventListener("unhandledrejection", onUnhandledRejection);
}

/**
 * Fetch and start the Sentry SDK, then replay everything buffered since
 * `initObservability()`. Call this AFTER the first render — on idle from here,
 * so the ~87 KB chunk never contends with the initial paint.
 */
export function loadObservability(): void {
  const dsn = configuredDsn;
  if (state !== "pending" || !dsn) return;

  whenIdle(() => {
    void import("./observability-sentry")
      .then((module) => adoptSentry(module, dsn))
      .catch(() => {
        // Chunk failed to load (offline, blocked by an extension). Reporting is
        // off for this session; release the buffer and the listeners.
        shutDown();
      });
  });
}
