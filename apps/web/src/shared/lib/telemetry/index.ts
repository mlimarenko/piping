/**
 * Small, statically imported telemetry facade.
 *
 * The RUM implementation is a lazy chunk so it never grows the initial bundle.
 * This module is the only import call sites need; it loads and starts collection
 * only when deployment policy and the browser choice allow it.
 */
import { isTelemetryAllowed, loadTelemetryPolicy, subscribeTelemetryConsent } from "./consent";
import { routeSegment } from "./events";
import type { ProductEvent, TelemetryEvent } from "./events";
import type * as RumModule from "./rum";

let rum: typeof RumModule | null = null;
let loading: Promise<typeof RumModule> | null = null;
let pending: TelemetryEvent[] = [];
let subscribed = false;

const MAX_PENDING = 40;

/** Load policy without delaying the app; reconcile changes while the chunk loads. */
export function initTelemetry(): void {
  if (!subscribed) {
    subscribeTelemetryConsent(() => startTelemetryIfAllowed());
    subscribed = true;
  }
  void loadTelemetryPolicy();
}

function loadRum(): Promise<typeof RumModule> {
  loading ??= import("./rum").then((module) => {
    rum = module;
    return module;
  });
  return loading;
}

/** Start collection when allowed; a withheld choice leaves it unloaded. */
export function startTelemetryIfAllowed(): void {
  if (!isTelemetryAllowed()) {
    rum?.stopTelemetry();
    pending = [];
    return;
  }
  void loadRum()
    .then((module) => {
      if (!isTelemetryAllowed()) return;
      module.startTelemetry();
      if (pending.length > 0) {
        const buffered = pending;
        pending = [];
        module.submitEvents(buffered);
      }
    })
    .catch(() => {
      // An unavailable telemetry chunk must never affect the application.
    });
}

function buffer(event: TelemetryEvent): void {
  if (pending.length < MAX_PENDING) pending.push(event);
}

/** Record one allow-listed product event when collection is enabled. */
export function trackProductEvent(event: ProductEvent): void {
  if (!isTelemetryAllowed()) return;
  if (rum) {
    rum.trackProductEvent(event);
    return;
  }
  buffer({ type: "product_event", event });
}

/** Count a browser-observed 5xx when collection is enabled. */
export function trackApiFailure(path: string, status: number): void {
  if (!isTelemetryAllowed() || status < 500) return;
  if (rum) {
    rum.trackApiFailure(path, status);
    return;
  }
  // Normalise before buffering: a raw path must never wait in memory or reach
  // the relay even though the backend would bound it again.
  buffer({ type: "api_failure", route: routeSegment(path), status });
}
