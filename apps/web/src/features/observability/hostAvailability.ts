import type { HostLabelEntry, HostLabelMetric } from "@/features/observability/api";
import {
  hostLabelMetricDescriptor,
  hostLabelMetricItem,
} from "@/features/observability/hostLabelProjection";
import {
  metricHasValue,
  metricValueSummary,
} from "@/features/observability/metricHelpers";
import { metricCardStatus } from "@/features/observability/metricDisplay";

export type HostAvailabilityState = "up" | "down" | "unknown" | "stale";

const AVAILABILITY_SEMANTIC = "availability";

export const STATE_DOT: Record<HostAvailabilityState, string> = {
  up: "bg-status-positive",
  down: "bg-status-negative",
  unknown: "bg-muted-foreground/45",
  stale: "bg-status-warning",
};

export const STATE_PILL: Record<HostAvailabilityState, string> = {
  up: "border-status-positive/35 bg-status-positive/10 text-status-positive",
  down: "border-status-negative/45 bg-status-negative/10 text-status-negative",
  unknown: "border-border/40 bg-muted/30 text-muted-foreground",
  stale: "border-status-warning/45 bg-status-warning/10 text-status-warning",
};

export function findAvailabilityMetric(entry: Pick<HostLabelEntry, "metrics"> | undefined): HostLabelMetric | undefined {
  return entry?.metrics?.find((metric) => metric.semantic === AVAILABILITY_SEMANTIC);
}

export function resolveHostAvailabilityState(
  entry: Pick<HostLabelEntry, "metrics" | "stale"> | undefined,
): HostAvailabilityState {
  const metric = findAvailabilityMetric(entry);
  if (!metric) return "unknown";
  if (entry?.stale) return "stale";

  const item = hostLabelMetricItem(metric);
  if (!metricHasValue(item)) return "unknown";

  const descriptor = hostLabelMetricDescriptor(metric);
  const summary = metricValueSummary(item, { value_type: metric.value_type, unit: metric.unit });
  const status = metricCardStatus(descriptor, item, summary);

  if (status === "normal") return "up";
  if (status === "critical") return "down";
  return "unknown";
}
