import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
  ServiceDiscoveryHostPreview,
} from "@/features/observability/api";

type MetricSourceInfo = ServiceDiscoveryHostPreview["metric_sources"][number];

export const metricKey = (metric: MonitoringMetricDescriptor) =>
  `${metric.source_type ?? "prometheus"}:${metric.name}:${metric.query}`;

function isPrometheusCompatible(sourceType: string): boolean {
  return sourceType === "prometheus" || sourceType === "victoriametrics";
}

export function metricSourceType(metric: MonitoringMetricDescriptor): string {
  return (metric.source_type ?? "prometheus").toLowerCase();
}

export function sourceTypesMatch(sourceType: string, metricType: string): boolean {
  if (sourceType === metricType) return true;
  return isPrometheusCompatible(sourceType) && isPrometheusCompatible(metricType);
}

export function metricHasResolvedSource(
  metric: MonitoringMetricDescriptor,
  metricSources: MetricSourceInfo[],
): boolean {
  return metricSources.some((source) =>
    sourceTypesMatch((source.source_type ?? "prometheus").toLowerCase(), metricSourceType(metric)),
  );
}

export function filterMetricsWithResolvedSources(
  metrics: MonitoringMetricDescriptor[],
  metricSources: MetricSourceInfo[],
): MonitoringMetricDescriptor[] {
  return metrics.filter((metric) => metricHasResolvedSource(metric, metricSources));
}

/**
 * Shared value→unit-interval normalization for the sparkline and the history
 * chart. Returns a `normalize(value)` mapping each value into `[0, 1]`
 * (0 = series min, 1 = series max), so both the compact sparkline path and the
 * larger chart derive identical vertical scaling from one source of truth. The
 * per-widget coordinate mapping (viewBox geometry) stays in each component.
 */
export function sparklineValueScale(values: number[]): (value: number) => number {
  const min = values.length > 0 ? Math.min(...values) : 0;
  const max = values.length > 0 ? Math.max(...values) : 0;
  const span = max - min || 1;
  return (value: number) => (value - min) / span;
}

export function parseMetricNumericValue(value: unknown): number | null {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Number.parseFloat(value);
    return Number.isFinite(parsed) ? parsed : null;
  }
  return null;
}

function normalizeToken(value: unknown): string {
  return typeof value === "string" ? value.trim().toLowerCase() : "";
}

function unknownValueSummary(value: unknown): string {
  if (value === undefined || value === null) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return value.toString();
  }
  if (typeof value === "symbol") return value.description ?? "symbol";
  try {
    return JSON.stringify(value);
  } catch {
    return Object.prototype.toString.call(value);
  }
}

function formatDecimal(value: number): string {
  const abs = Math.abs(value);
  if (abs === 0) return "0";
  if (abs >= 100) return value.toFixed(0);
  if (abs >= 10) return value.toFixed(1).replace(/\.0$/, "");
  if (abs >= 1) return value.toFixed(2).replace(/0+$/, "").replace(/\.$/, "");
  return value.toFixed(3).replace(/0+$/, "").replace(/\.$/, "");
}

function formatBytes(value: number, suffix = ""): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
  let size = Math.abs(value);
  let unitIndex = 0;
  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }
  const signed = value < 0 ? -size : size;
  const decimals = signed >= 100 || unitIndex === 0 ? 0 : signed >= 10 ? 1 : 2;
  return `${signed.toFixed(decimals).replace(/\.0+$/, "")} ${units[unitIndex]}${suffix}`;
}

function formatBitRate(value: number): string {
  const units = ["bit/s", "Kbit/s", "Mbit/s", "Gbit/s", "Tbit/s"];
  let size = Math.abs(value);
  let unitIndex = 0;
  while (size >= 1000 && unitIndex < units.length - 1) {
    size /= 1000;
    unitIndex += 1;
  }
  const signed = value < 0 ? -size : size;
  return `${formatDecimal(signed)} ${units[unitIndex]}`;
}

/** Replace the row with the same id, leaving the rest untouched. */
export function replaceById<T extends { id: string }>(rows: T[], saved: T): T[] {
  return rows.map((row) => (row.id === saved.id ? saved : row));
}

function formatPercent(value: number): string {
  return `${value.toFixed(Math.abs(value) >= 10 ? 0 : 1).replace(/\.0$/, "")}%`;
}

function formatDurationSeconds(value: number): string {
  const totalSeconds = Math.max(0, Math.floor(value));
  const days = Math.floor(totalSeconds / 86_400);
  const hours = Math.floor((totalSeconds % 86_400) / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  if (days > 0) return `${days}d ${hours}h`;
  if (hours > 0) return `${hours}h ${minutes}m`;
  if (minutes > 0) return `${minutes}m ${seconds}s`;
  return `${seconds}s`;
}

function valueTypeFromUnit(unit: string | null | undefined): string | null {
  const rawUnit = typeof unit === "string" ? unit.trim() : "";
  const normalized = normalizeToken(rawUnit);
  if (!normalized) return null;
  if (normalized === "%") return "percent";
  if (["s", "sec", "secs", "second", "seconds"].includes(normalized)) return "duration";
  if (["ms", "millisecond", "milliseconds"].includes(normalized)) return "milliseconds";
  if (rawUnit === "B" || ["byte", "bytes"].includes(normalized)) return "bytes";
  if (rawUnit === "B/s" || ["byte/s", "bytes/s", "byte/sec", "bytes/sec"].includes(normalized)) {
    return "bytes_per_second";
  }
  if (["bit/s", "bits/s", "bps"].includes(normalized)) return "bits_per_second";
  if (["state", "status", "bool", "boolean"].includes(normalized)) return "state";
  return null;
}

function effectiveValueType(
  item: LatestMetricItem,
  metric?: Pick<MonitoringMetricDescriptor, "value_type" | "unit">,
): string {
  const explicit = normalizeToken(metric?.value_type);
  if (explicit && explicit !== "auto") return explicit;
  const latestExplicit = normalizeToken(item.value_type);
  if (latestExplicit && latestExplicit !== "auto") return latestExplicit;
  return valueTypeFromUnit(metric?.unit ?? item.unit) ?? "auto";
}

function formatByUnit(value: number, unit: string | null | undefined): string | null {
  const rawUnit = typeof unit === "string" ? unit.trim() : "";
  const normalized = normalizeToken(rawUnit);
  if (!normalized) return null;
  if (normalized === "%") return formatPercent(value);
  if (["s", "sec", "secs", "second", "seconds"].includes(normalized)) return formatDurationSeconds(value);
  if (["ms", "millisecond", "milliseconds"].includes(normalized)) return formatDurationSeconds(value / 1000);
  if (rawUnit === "B" || ["byte", "bytes"].includes(normalized)) return formatBytes(value);
  if (rawUnit === "B/s" || ["byte/s", "bytes/s", "byte/sec", "bytes/sec"].includes(normalized)) {
    return formatBytes(value, "/s");
  }
  if (["bit/s", "bits/s", "bps"].includes(normalized)) return formatBitRate(value);
  if (["state", "status", "bool", "boolean", "load", "count", "number"].includes(normalized)) {
    return formatDecimal(value);
  }
  return `${formatDecimal(value)} ${rawUnit}`;
}

export function rawMetricValue(item: LatestMetricItem): unknown {
  const zabbix = item.value as
    | {
        provider?: string;
        value?: unknown;
        item?: { lastvalue?: unknown };
      }
    | undefined;
  if (zabbix?.provider === "zabbix") {
    return zabbix.value ?? zabbix.item?.lastvalue;
  }

  const body = item.value as
    | {
        data?: {
          result?: Array<{ value?: [number, string]; metric?: Record<string, string> }>;
        };
      }
    | undefined;
  const first = body?.data?.result?.[0];
  if (first?.value?.[1] !== undefined) return first.value[1];
  return item.value;
}

export function metricHasValue(item: LatestMetricItem | undefined): boolean {
  if (!item || item.error) return false;
  const zabbix = item.value as
    | {
        provider?: string;
        value?: unknown;
        item?: { lastvalue?: unknown };
      }
    | undefined;
  if (zabbix?.provider === "zabbix") {
    const value = zabbix.value ?? zabbix.item?.lastvalue;
    return value !== undefined && value !== null && value !== "";
  }

  const body = item.value as
    | {
        data?: {
          result?: Array<{ value?: [number, string] }>;
        };
      }
    | undefined;
  if (Array.isArray(body?.data?.result)) {
    return body.data.result.some((row) => row.value?.[1] !== undefined && row.value[1] !== "");
  }

  const raw = rawMetricValue(item);
  return raw !== undefined && raw !== null && raw !== "";
}

export function metricValueSummary(
  item: LatestMetricItem,
  metric?: Pick<MonitoringMetricDescriptor, "value_type" | "unit">,
): string {
  if (item.error) return item.error;
  if (!metricHasValue(item)) return "—";
  const raw = rawMetricValue(item);
  const numeric = parseMetricNumericValue(raw);
  const unit = metric?.unit ?? item.unit;
  switch (effectiveValueType(item, metric)) {
    case "bytes_per_second":
      return numeric === null ? unknownValueSummary(raw) : formatBytes(numeric, "/s");
    case "bits_per_second":
      return numeric === null ? unknownValueSummary(raw) : formatBitRate(numeric);
    case "milliseconds":
      return numeric === null ? unknownValueSummary(raw) : formatDurationSeconds(numeric / 1000);
    case "bytes":
      return numeric === null ? unknownValueSummary(raw) : formatBytes(numeric);
    case "duration":
      return numeric === null ? unknownValueSummary(raw) : formatDurationSeconds(numeric);
    case "percent":
      return numeric === null ? unknownValueSummary(raw) : formatPercent(numeric);
    case "state":
      if (raw === "1" || raw === 1 || raw === true) return "up";
      if (raw === "0" || raw === 0 || raw === false) return "down";
      return unknownValueSummary(raw);
    case "timestamp":
      if (numeric !== null) return new Date(numeric * 1000).toLocaleString();
      return unknownValueSummary(raw);
    case "log":
    case "text":
      return unknownValueSummary(raw);
    default:
      if (numeric !== null) return formatByUnit(numeric, unit) ?? formatDecimal(numeric);
      return unknownValueSummary(raw);
  }
}

// ── Metric category grouping ─────────────────────────────
//
// The inspector buckets metrics by `category` so an operator with many cards
// can collapse whole sections. Kept here (single-source-of-truth) so the
// preview section and any future consumer (workbench preview, dialogs) share
// the same insertion-order semantics and uncategorized handling.

export const UNCATEGORIZED_CATEGORY_KEY = "__uncategorized__";

export interface MetricCategoryGroup {
  /** Stable storage key: the category name verbatim, or the uncategorized sentinel. */
  key: string;
  /** Display title for the group header. */
  title: string;
  metrics: MonitoringMetricDescriptor[];
}

/**
 * Buckets metrics by `category` in first-seen order so the inspector preserves
 * whatever order the metric-set assignment defined. Metrics with NULL / blank
 * category fall into the `__uncategorized__` bucket; callers decide whether to
 * render that bucket's title as "Other" or to flatten it. The uncategorized
 * bucket is always pinned last so operators see named categories first and
 * a stable "Other" tail, regardless of insertion order.
 */
export function groupMetricsByCategory(
  metrics: MonitoringMetricDescriptor[],
  uncategorizedTitle: string,
): MetricCategoryGroup[] {
  const byKey = new Map<string, MetricCategoryGroup>();
  for (const metric of metrics) {
    const raw = (metric.category ?? "").trim();
    const key = raw === "" ? UNCATEGORIZED_CATEGORY_KEY : raw;
    const title = raw === "" ? uncategorizedTitle : raw;
    let bucket = byKey.get(key);
    if (!bucket) {
      bucket = { key, title, metrics: [] };
      byKey.set(key, bucket);
    }
    bucket.metrics.push(metric);
  }
  const groups = [...byKey.values()];
  const uncategorized = groups.find((g) => g.key === UNCATEGORIZED_CATEGORY_KEY);
  if (!uncategorized) return groups;
  const named = groups.filter((g) => g.key !== UNCATEGORIZED_CATEGORY_KEY);
  return [...named, uncategorized];
}
