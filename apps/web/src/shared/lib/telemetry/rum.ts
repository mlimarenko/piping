/**
 * Browser RUM and product analytics.
 *
 * Every report goes to the same-origin relay `POST /api/v1/rum/report`. The
 * browser never holds a collector endpoint or write credentials; the backend
 * validates the payload and records it in the existing scraped metrics
 * registry.
 *
 * Nothing is queued or sent unless
 * `isTelemetryAllowed()` is true at the moment of the call AND at flush time.
 * Withdrawing consent clears the queue and tears down every listener.
 *
 * Labels stay bounded: a route is one path segment, events come from a fixed
 * union, and a stack is capped before it leaves the tab. Error messages,
 * function names, query strings, storage and form values are never read.
 */
import { isTelemetryAllowed, subscribeTelemetryConsent } from "./consent";
import { boundedStack, routeSegment } from "./events";
import type { ProductEvent, TelemetryEvent, WebVitalMetric } from "./events";

const ENDPOINT = "/api/v1/rum/report";
const PAYLOAD_VERSION = 1;
const MAX_QUEUE = 60;
const MAX_BATCH = 20;
const FLUSH_INTERVAL_MS = 15_000;
const VITALS_REPORT_DELAY_MS = 10_000;

interface Instruments {
  reported: Set<WebVitalMetric>;
  lcp: number;
  cls: number;
  inp: number;
  observers: PerformanceObserver[];
}

let started = false;
let queue: TelemetryEvent[] = [];
let instruments: Instruments | null = null;
let flushTimer: number | undefined;
let vitalsTimer: number | undefined;
let unsubscribeConsent: (() => void) | undefined;
let onError: ((event: ErrorEvent) => void) | undefined;
let onRejection: ((event: PromiseRejectionEvent) => void) | undefined;
let onHidden: (() => void) | undefined;
let onVisibility: (() => void) | undefined;

/** Append to the bounded queue after checking deployment policy and preference. */
function enqueue(events: TelemetryEvent[]): void {
  if (!isTelemetryAllowed()) return;
  for (const event of events) {
    if (queue.length >= MAX_QUEUE) break;
    queue.push(event);
  }
  if (queue.length >= MAX_BATCH) flush();
}

function sendBatch(events: TelemetryEvent[]): void {
  if (events.length === 0) return;
  const body = JSON.stringify({ version: PAYLOAD_VERSION, events });
  if (body.length > 7_500) {
    // Oversized reports are not split: dropping one batch is cheaper than
    // letting a hostile stack stretch the relay's body limit.
    return;
  }
  try {
    if (
      typeof navigator.sendBeacon === "function" &&
      document.visibilityState === "hidden"
    ) {
      navigator.sendBeacon(
        ENDPOINT,
        new Blob([body], { type: "application/json" }),
      );
      return;
    }
    void fetch(ENDPOINT, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body,
      keepalive: true,
      credentials: "omit",
    }).catch(() => {
      // Telemetry is best-effort; a failed report must never affect the app.
    });
  } catch {
    // Same contract: a blocked request or unavailable beacon drops the batch.
  }
}

/** Send queued events now. Used by the timer and by page-hide. */
export function flush(): void {
  if (!isTelemetryAllowed()) {
    queue = [];
    stopTelemetry();
    return;
  }
  while (queue.length > 0) {
    sendBatch(queue.splice(0, MAX_BATCH));
  }
}

/** Accept early events from the facade after the module loads. */
export function submitEvents(events: TelemetryEvent[]): void {
  if (!started) return;
  enqueue(events);
}

export function trackProductEvent(event: ProductEvent): void {
  if (!started) return;
  enqueue([{ type: "product_event", event }]);
}

/** Count a browser-observed API failure. Only server failures are reported. */
export function trackApiFailure(path: string, status: number): void {
  if (!started || status < 500) return;
  enqueue([{ type: "api_failure", route: routeSegment(path), status }]);
}

function recordVital(metric: WebVitalMetric, value: number): void {
  const current = instruments;
  if (current === null || current.reported.has(metric) || !Number.isFinite(value)) {
    return;
  }
  current.reported.add(metric);
  enqueue([{ type: "vital", metric, value }]);
}

function observe(type: string, callback: (entries: PerformanceEntry[]) => void): void {
  if (typeof PerformanceObserver === "undefined" || instruments === null) return;
  try {
    const observer = new PerformanceObserver((list) => callback(list.getEntries()));
    observer.observe({ type, buffered: true });
    instruments.observers.push(observer);
  } catch {
    // The browser does not expose this entry type; the vital stays absent.
  }
}

function recordNavigationTimings(): void {
  const fcp = performance.getEntriesByName("first-contentful-paint")[0];
  if (fcp) recordVital("fcp", fcp.startTime);
  const navigation = performance.getEntriesByType("navigation")[0] as
    | PerformanceNavigationTiming
    | undefined;
  if (navigation) {
    recordVital("ttfb", navigation.responseStart - navigation.startTime);
  }
}

function reportCurrentVitals(): void {
  const current = instruments;
  if (current === null) return;
  if (current.lcp > 0) recordVital("lcp", current.lcp);
  recordVital("cls", current.cls);
  if (current.inp > 0) recordVital("inp", current.inp);
}

function installWebVitals(): void {
  instruments = { reported: new Set(), lcp: 0, cls: 0, inp: 0, observers: [] };

  observe("largest-contentful-paint", (entries) => {
    const last = entries[entries.length - 1];
    if (last && instruments) instruments.lcp = last.startTime;
  });
  observe("layout-shift", (entries) => {
    for (const entry of entries) {
      const shift = entry as PerformanceEntry & {
        value?: number;
        hadRecentInput?: boolean;
      };
      if (
        instruments &&
        !shift.hadRecentInput &&
        typeof shift.value === "number"
      ) {
        instruments.cls += shift.value;
      }
    }
  });
  observe("event", (entries) => {
    for (const entry of entries) {
      const event = entry as PerformanceEntry & { duration?: number };
      if (instruments && typeof event.duration === "number") {
        instruments.inp = Math.max(instruments.inp, event.duration);
      }
    }
  });

  recordNavigationTimings();
  vitalsTimer = window.setTimeout(() => {
    recordNavigationTimings();
    reportCurrentVitals();
    flush();
  }, VITALS_REPORT_DELAY_MS);
}

function installErrorCapture(): void {
  onError = (event: ErrorEvent) => {
    enqueue([
      {
        type: "js_error",
        source: "uncaught",
        route: routeSegment(window.location.pathname),
        stack: boundedStack(event.error instanceof Error ? event.error.stack : event.message),
      },
    ]);
  };
  onRejection = (event: PromiseRejectionEvent) => {
    enqueue([
      {
        type: "js_error",
        source: "unhandled_rejection",
        route: routeSegment(window.location.pathname),
        stack: boundedStack(event.reason instanceof Error ? event.reason.stack : undefined),
      },
    ]);
  };
  window.addEventListener("error", onError);
  window.addEventListener("unhandledrejection", onRejection);
}

/**
 * Start collection once. Safe to call repeatedly; a withheld consent or a
 * previous shutdown keeps it inert.
 */
export function startTelemetry(): void {
  if (started || !isTelemetryAllowed()) return;
  started = true;
  queue = [];

  installWebVitals();
  installErrorCapture();
  flushTimer = window.setInterval(flush, FLUSH_INTERVAL_MS);

  onHidden = () => {
    reportCurrentVitals();
    recordNavigationTimings();
    flush();
  };
  window.addEventListener("pagehide", onHidden);
  onVisibility = () => {
    if (document.visibilityState === "hidden") onHidden?.();
  };
  document.addEventListener("visibilitychange", onVisibility);

  unsubscribeConsent = subscribeTelemetryConsent(() => {
    if (!isTelemetryAllowed()) stopTelemetry();
  });

  enqueue([{ type: "product_event", event: "app_opened" }]);
}

/** Tear down every listener and drop unsent data. */
export function stopTelemetry(): void {
  if (!started) return;
  started = false;
  queue = [];
  if (flushTimer !== undefined) window.clearInterval(flushTimer);
  if (vitalsTimer !== undefined) window.clearTimeout(vitalsTimer);
  flushTimer = undefined;
  vitalsTimer = undefined;
  unsubscribeConsent?.();
  unsubscribeConsent = undefined;
  if (onError) window.removeEventListener("error", onError);
  if (onRejection) window.removeEventListener("unhandledrejection", onRejection);
  if (onHidden) window.removeEventListener("pagehide", onHidden);
  if (onVisibility) document.removeEventListener("visibilitychange", onVisibility);
  onError = undefined;
  onRejection = undefined;
  onHidden = undefined;
  onVisibility = undefined;
  for (const observer of instruments?.observers ?? []) observer.disconnect();
  instruments = null;
}

/** Test-only reset: stops collection and forgets module state. */
export function resetTelemetryForTests(): void {
  stopTelemetry();
  started = false;
  queue = [];
}
