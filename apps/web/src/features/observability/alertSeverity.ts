import type { HostAlert, HostLabelMetric } from "@/features/observability/api";
import { summarizeHostLabelEntry } from "./hostObservabilitySummary";
import type { MetricCardStatus } from "./metricDisplay";

// Active alerts are at least a warning, so an unrecognised severity never drops
// to a healthy tone — it falls back to `warning`. Severity strings are matched
// structurally against small, vendor-neutral synonym sets (Alertmanager /
// vmalert / Zabbix all use overlapping vocabularies) rather than any single
// provider's full ontology.
export type AlertStatus = Exclude<MetricCardStatus, "neutral">;

const CRITICAL_SEVERITIES = new Set([
  "critical",
  "crit",
  "fatal",
  "disaster",
  "page",
  "emergency",
  "error",
  "down",
]);

const WARNING_SEVERITIES = new Set([
  "warning",
  "warn",
  "high",
  "average",
  "degraded",
  "minor",
  "major",
]);

const INFO_SEVERITIES = new Set([
  "info",
  "information",
  "informational",
  "notice",
  "low",
  "none",
]);

const STATUS_ORDER: Record<AlertStatus, number> = {
  normal: 0,
  warning: 1,
  critical: 2,
};

function normalizeSeverity(value: string | null | undefined): string {
  return typeof value === "string" ? value.trim().toLowerCase() : "";
}

export function alertSeverityStatus(severity: string | null | undefined): AlertStatus {
  const token = normalizeSeverity(severity);
  if (CRITICAL_SEVERITIES.has(token)) return "critical";
  if (WARNING_SEVERITIES.has(token)) return "warning";
  if (INFO_SEVERITIES.has(token)) return "normal";
  // A firing alert with an unknown severity is at least a warning.
  return "warning";
}

function worse(a: AlertStatus, b: AlertStatus): AlertStatus {
  return STATUS_ORDER[a] >= STATUS_ORDER[b] ? a : b;
}

export interface AlertBadgeRollup {
  count: number;
  status: AlertStatus | null;
  alerts: HostAlert[];
}

/**
 * Combines explicit firing alerts with the threshold rollup derived from the
 * host's list-label metrics. The badge status is the worst severity across
 * mapped alert severities and the metric rollup state; the count is the number
 * of firing alerts. Returns `status: null` when there is nothing worth
 * surfacing (no alerts and a healthy/no-data rollup), so the caller renders
 * nothing.
 */
export function rollupHostAlerts(
  alerts: HostAlert[] | undefined,
  metrics: HostLabelMetric[] | undefined,
  stale: boolean,
): AlertBadgeRollup {
  const list = alerts ?? [];
  const summary = summarizeHostLabelEntry({ metrics: metrics ?? [], stale });

  let status: AlertStatus | null = null;
  for (const alert of list) {
    const mapped = alertSeverityStatus(alert.severity);
    status = status ? worse(status, mapped) : mapped;
  }
  // Only a metric threshold breach (warning/critical) escalates the badge; a
  // pure no-data/unknown rollup is treated as nothing to surface to avoid
  // lighting up every host that merely has a metric without a current value.
  if (summary.critical > 0) status = status ? worse(status, "critical") : "critical";
  else if (summary.warning > 0) status = status ? worse(status, "warning") : "warning";

  // A worst-severity of `normal` means every contribution mapped to info/healthy
  // (e.g. only `info`/`low` alerts and no threshold breach). A green "alert" pill
  // reads as contradictory, so surface nothing — the alerts stay reachable in the
  // hover card. Anything worth a badge is at least `warning`.
  if (status === "normal") status = null;

  return { count: list.length, status, alerts: list };
}

export function alertLine(alert: HostAlert): string {
  const name = alert.name?.trim() || "alert";
  const severity = normalizeSeverity(alert.severity) || "unknown";
  const summary = alert.summary?.trim();
  return summary ? `${name} — ${severity}: ${summary}` : `${name} — ${severity}`;
}
