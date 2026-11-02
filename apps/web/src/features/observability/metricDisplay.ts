import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import { parseMetricNumericValue, rawMetricValue } from "./metricHelpers";

export type MetricCardStatus = "neutral" | "normal" | "warning" | "critical";

type JsonRecord = Record<string, unknown>;
type NumericDirection = "higher_is_worse" | "lower_is_worse";

interface NumericStatusRule {
  direction: NumericDirection;
  warning: number | null;
  critical: number | null;
}

const DEFAULT_STATE_RULES: Record<Exclude<MetricCardStatus, "neutral">, string[]> = {
  normal: ["up", "ok", "active", "healthy", "success", "true", "1"],
  warning: ["unknown", "pending", "degraded", "warning", "warn"],
  critical: ["down", "failed", "fail", "error", "critical", "false", "0"],
};

function asRecord(value: unknown): JsonRecord | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonRecord)
    : null;
}

function asNumber(value: unknown): number | null {
  return parseMetricNumericValue(value);
}

function asStringArray(value: unknown): string[] {
  if (Array.isArray(value)) {
    return value
      .map((item) => (typeof item === "string" || typeof item === "number" || typeof item === "boolean" ? String(item) : ""))
      .map(normalizeToken)
      .filter(Boolean);
  }
  if (typeof value === "string") {
    return value
      .split(",")
      .map(normalizeToken)
      .filter(Boolean);
  }
  return [];
}

function normalizeToken(value: unknown): string {
  if (typeof value === "string" || typeof value === "number" || typeof value === "boolean") {
    return String(value).trim().toLowerCase();
  }
  return "";
}

function statusConfig(metric: MonitoringMetricDescriptor): JsonRecord | null {
  const options = asRecord(metric.options);
  const display = asRecord(options?.display);
  const status = asRecord(display?.status);
  return status ?? asRecord(display?.thresholds) ?? asRecord(options?.status) ?? null;
}

export function metricStatusMode(options: unknown): string {
  const display = asRecord(asRecord(options)?.display);
  const status = asRecord(display?.status);
  const mode = status?.mode;
  return typeof mode === "string" && mode.trim() ? mode : "auto";
}

function numericRule(config: JsonRecord | null, metric: MonitoringMetricDescriptor): NumericStatusRule | null {
  if (config) {
    const mode = normalizeToken(config.mode);
    if (mode && mode !== "auto" && mode !== "numeric") return null;
    const warning = asNumber(config.warning);
    const critical = asNumber(config.critical);
    if (mode === "numeric" || warning !== null || critical !== null) {
      const direction = normalizeToken(config.direction);
      return {
        direction: direction === "lower_is_worse" ? "lower_is_worse" : "higher_is_worse",
        warning,
        critical,
      };
    }
  }

  const valueType = normalizeToken(metric.value_type);
  const unit = normalizeToken(metric.unit);
  if (valueType === "percent" || unit === "%") {
    return { direction: "higher_is_worse", warning: 80, critical: 90 };
  }
  return null;
}

function stateStatus(
  config: JsonRecord | null,
  raw: unknown,
  summary: string,
): MetricCardStatus | null {
  const mode = normalizeToken(config?.mode);
  if (mode && mode !== "auto" && mode !== "state") return null;
  const candidates = [raw, summary].map(normalizeToken).filter(Boolean);
  if (candidates.length === 0) return null;

  const rules = {
    normal: asStringArray(config?.normal).length > 0 ? asStringArray(config?.normal) : DEFAULT_STATE_RULES.normal,
    warning: asStringArray(config?.warning).length > 0 ? asStringArray(config?.warning) : DEFAULT_STATE_RULES.warning,
    critical:
      asStringArray(config?.critical).length > 0 ? asStringArray(config?.critical) : DEFAULT_STATE_RULES.critical,
  };

  if (candidates.some((value) => rules.critical.includes(value))) return "critical";
  if (candidates.some((value) => rules.warning.includes(value))) return "warning";
  if (candidates.some((value) => rules.normal.includes(value))) return "normal";
  return null;
}

function numericStatus(rule: NumericStatusRule, numeric: number | null): MetricCardStatus | null {
  if (numeric === null) return null;
  const { warning, critical, direction } = rule;
  if (direction === "lower_is_worse") {
    if (critical !== null && numeric <= critical) return "critical";
    if (warning !== null && numeric <= warning) return "warning";
    return warning !== null || critical !== null ? "normal" : null;
  }
  if (critical !== null && numeric >= critical) return "critical";
  if (warning !== null && numeric >= warning) return "warning";
  return warning !== null || critical !== null ? "normal" : null;
}

export function metricCardStatus(
  metric: MonitoringMetricDescriptor,
  latest: LatestMetricItem | undefined,
  summary: string,
): MetricCardStatus {
  if (latest?.error) return "critical";

  const config = statusConfig(metric);
  const mode = normalizeToken(config?.mode);
  if (mode === "none") return "neutral";

  const raw = latest ? rawMetricValue(latest) : undefined;
  if (mode === "state" || ((!mode || mode === "auto") && normalizeToken(metric.value_type) === "state")) {
    return stateStatus(config, raw, summary) ?? "neutral";
  }

  const rule = numericRule(config, metric);
  const numeric = parseMetricNumericValue(raw ?? summary);
  return (rule ? numericStatus(rule, numeric) : null) ?? "neutral";
}
