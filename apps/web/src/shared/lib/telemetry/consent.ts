/**
 * Browser telemetry consent gate.
 *
 * Deployment policy and the browser's explicit choice jointly govern collection.
 */
const CONSENT_KEY = "piping.telemetry.consent";

export type TelemetryConsent = "granted" | "denied" | "unset";

type Listener = (consent: TelemetryConsent) => void;
type TelemetryState = TelemetryConsent | "pending" | "disabled";

const listeners = new Set<Listener>();
let cached: TelemetryConsent | undefined;
let policy: { enabled: boolean; defaultEnabled: boolean } | undefined;
let policyRequest: Promise<void> | undefined;

function read(): TelemetryConsent {
  try {
    const value = window.localStorage.getItem(CONSENT_KEY);
    return value === "granted" || value === "denied" ? value : "unset";
  } catch {
    // Storage can be unavailable in private modes; treat it as "not chosen".
    return "unset";
  }
}

/** Current explicit browser choice, independent of deployment defaults. */
export function telemetryConsent(): TelemetryConsent {
  cached ??= read();
  return cached;
}

/** A primitive snapshot stays stable between changes for React subscribers. */
export function telemetryState(): TelemetryState {
  if (!policy) return "pending";
  if (!policy.enabled) return "disabled";
  const consent = telemetryConsent();
  return consent === "unset" && policy.defaultEnabled ? "granted" : consent;
}

/** Fetch the same-origin deployment policy once; failure keeps collection off. */
export function loadTelemetryPolicy(): Promise<void> {
  policyRequest ??= (async () => {
    try {
      const response = await fetch("/api/v1/rum/config", {
        credentials: "omit",
        cache: "no-store",
        signal: AbortSignal.timeout(15_000),
      });
      if (!response.ok) throw new Error("Telemetry policy unavailable");
      const data: unknown = await response.json();
      if (
        typeof data !== "object" || data === null ||
        !("enabled" in data) || typeof data.enabled !== "boolean" ||
        !("default_enabled" in data) || typeof data.default_enabled !== "boolean"
      ) throw new Error("Invalid telemetry policy");
      policy = { enabled: data.enabled, defaultEnabled: data.default_enabled };
    } catch {
      // A missing or invalid policy must never enable collection or block startup.
      policy = { enabled: false, defaultEnabled: false };
    }
    notify(telemetryConsent());
  })();
  return policyRequest;
}

function notify(next: TelemetryConsent): void {
  for (const listener of listeners) listener(next);
}

/** Record an explicit choice and notify mounted controls. */
export function setTelemetryConsent(next: "granted" | "denied"): void {
  cached = next;
  try {
    window.localStorage.setItem(CONSENT_KEY, next);
  } catch {
    // The in-memory choice still applies for this page session.
  }
  notify(next);
}

export function subscribeTelemetryConsent(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Whether deployment policy and the saved browser choice allow collection. */
export function isTelemetryAllowed(): boolean {
  return telemetryState() === "granted";
}

/** Test-only reset: forget the cached policy and browser choice. */
export function resetTelemetryConsentForTests(): void {
  cached = undefined;
  policy = undefined;
  policyRequest = undefined;
}
