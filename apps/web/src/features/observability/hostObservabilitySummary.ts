import type {
  HostLabelEntry,
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import { metricCardStatus } from "./metricDisplay";
import { metricHasValue, metricValueSummary } from "./metricHelpers";
import { hostLabelMetricDescriptor, hostLabelMetricItem } from "./hostLabelProjection";

export type HostObservabilityState = "normal" | "warning" | "critical" | "unknown";

export interface HostObservabilitySummary {
  state: HostObservabilityState;
  total: number;
  ok: number;
  warning: number;
  critical: number;
  unknown: number;
  stale: boolean;
}

export function summarizeHostObservability(
  metrics: MonitoringMetricDescriptor[],
  latestByQuery: Map<string, LatestMetricItem>,
  stale: boolean,
): HostObservabilitySummary {
  let ok = 0;
  let warning = 0;
  let critical = 0;
  let unknown = 0;

  for (const metric of metrics) {
    const latest = latestByQuery.get(metric.query);
    if (!latest) {
      unknown += 1;
      continue;
    }

    const status = metricCardStatus(metric, latest, metricValueSummary(latest, metric));
    if (status === "critical") {
      critical += 1;
    } else if (status === "warning") {
      warning += 1;
    } else if (metricHasValue(latest)) {
      ok += 1;
    } else {
      unknown += 1;
    }
  }

  const state: HostObservabilityState =
    critical > 0 ? "critical" : warning > 0 ? "warning" : unknown > 0 ? "unknown" : "normal";

  return {
    state,
    total: metrics.length,
    ok,
    warning,
    critical,
    unknown,
    stale,
  };
}

/**
 * Rolls up the threshold state of a cached host-label entry by projecting its
 * `HostLabelMetric`s onto the descriptor/latest shape and running the same
 * `summarizeHostObservability` logic the inspector uses. Reused by the alert
 * badge and the hover card so the per-host status is derived in exactly one
 * place from exactly one conversion.
 */
export function summarizeHostLabelEntry(
  entry: Pick<HostLabelEntry, "metrics" | "stale"> | undefined,
): HostObservabilitySummary {
  const metrics = entry?.metrics ?? [];
  const descriptors = metrics.map(hostLabelMetricDescriptor);
  const latestByQuery = new Map<string, LatestMetricItem>(
    metrics.map((metric) => [metric.query, hostLabelMetricItem(metric)]),
  );
  return summarizeHostObservability(descriptors, latestByQuery, entry?.stale ?? false);
}
