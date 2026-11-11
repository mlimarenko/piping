import { useTranslation } from "react-i18next";

import type {
  HostLabelMetric,
  LatestMetricItem,
} from "@/features/observability/api";
import {
  metricHasValue,
  metricValueSummary,
  parseMetricNumericValue,
  rawMetricValue,
} from "@/features/observability/metricHelpers";
import {
  metricCardStatus,
  type MetricCardStatus,
} from "@/features/observability/metricDisplay";
import {
  hostLabelMetricDescriptor,
  hostLabelMetricItem,
} from "@/features/observability/hostLabelProjection";
import { cn } from "@/shared/lib/utils";

// Reuse the same CDS status tokens StatusChip / MetricWidgetCard use so colors
// stay consistent and dark-mode safe. No bespoke palette.
const STATUS_DOT: Record<MetricCardStatus, string> = {
  neutral: "bg-muted-foreground",
  normal: "bg-status-positive",
  warning: "bg-status-warning",
  critical: "bg-status-negative",
};

const STATUS_PILL: Record<MetricCardStatus, string> = {
  neutral: "border-border bg-muted text-muted-foreground",
  normal: "border-status-positive/35 bg-card text-status-positive",
  warning: "border-status-warning/45 bg-card text-status-warning",
  critical: "border-status-negative/45 bg-card text-status-negative",
};

const STATUS_BAR: Record<MetricCardStatus, string> = {
  neutral: "bg-muted-foreground/45",
  normal: "bg-status-positive",
  warning: "bg-status-warning",
  critical: "bg-status-negative",
};

type ListRender = "auto" | "dot" | "value" | "boolean" | "gauge";

function normalizeRender(value: string | null | undefined): ListRender {
  switch ((value ?? "").trim().toLowerCase()) {
    case "dot":
      return "dot";
    case "value":
      return "value";
    case "boolean":
      return "boolean";
    case "gauge":
      return "gauge";
    default:
      return "auto";
  }
}

function normalizeToken(value: string | null | undefined): string {
  return typeof value === "string" ? value.trim().toLowerCase() : "";
}

/**
 * `auto` derivation: state / boolean value types render as a boolean pill;
 * percent / gauge widgets render as a gauge bar; everything else is a value
 * pill. Mirrors how the inspector card picks a shape from the same fields.
 */
function resolveRender(render: ListRender, metric: HostLabelMetric): ListRender {
  if (render !== "auto") return render;
  const valueType = normalizeToken(metric.value_type);
  const widget = normalizeToken(metric.widget_type);
  if (valueType === "state" || valueType === "boolean" || widget === "badge") return "boolean";
  if (valueType === "percent" || widget === "gauge" || normalizeToken(metric.unit) === "%") {
    return "gauge";
  }
  return "value";
}

function metricGaugePercent(item: LatestMetricItem): number | null {
  const numeric = parseMetricNumericValue(rawMetricValue(item));
  if (numeric === null) return null;
  return Math.max(0, Math.min(100, numeric));
}

interface MetricLabelProps {
  metric: HostLabelMetric;
}

const MetricLabel = ({ metric }: MetricLabelProps) => {
  const { t } = useTranslation();
  const item = hostLabelMetricItem(metric);
  const descriptor = hostLabelMetricDescriptor(metric);
  const summary = metricValueSummary(item, { value_type: metric.value_type, unit: metric.unit });
  const status = metricCardStatus(descriptor, item, summary);
  const label = metric.label?.trim() || metric.query;
  const render = resolveRender(normalizeRender(metric.list_render), metric);

  if (!metricHasValue(item)) {
    const title = `${label}: ${t("observability.hostLabel.noValue", { defaultValue: "no data" })}`;
    return (
      <span
        className="inline-flex items-center gap-1 text-xs text-muted-foreground"
        title={title}
        aria-label={title}
      >
        <span className="h-1.5 w-1.5 rounded-full bg-muted-foreground/40" aria-hidden />
        <span aria-hidden>—</span>
      </span>
    );
  }

  const title = `${label}: ${summary}`;

  if (render === "dot") {
    return (
      <span
        className="inline-flex h-2 w-2 shrink-0 rounded-full"
        title={title}
        aria-label={title}
      >
        <span className={cn("h-full w-full rounded-full", STATUS_DOT[status])} aria-hidden />
      </span>
    );
  }

  if (render === "gauge") {
    // Only a percent-scaled metric maps cleanly to a 0–100 fill; a non-percent
    // gauge (load average, bytes) would draw a misleading bar, so show value only.
    const isPercentScaled =
      normalizeToken(metric.value_type) === "percent" || normalizeToken(metric.unit) === "%";
    const percent = isPercentScaled ? metricGaugePercent(item) : null;
    return (
      <span
        className={cn(
          "inline-flex max-w-[7rem] items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs font-medium leading-none",
          STATUS_PILL[status],
        )}
        title={title}
        aria-label={title}
      >
        {percent !== null && (
          <span className="h-1 w-6 overflow-hidden rounded-full bg-muted" aria-hidden>
            <span
              className={cn("block h-full rounded-full", STATUS_BAR[status])}
              style={{ width: `${percent}%` }}
            />
          </span>
        )}
        <span className="truncate tabular-nums">{summary}</span>
      </span>
    );
  }

  // `value` and `boolean` share the compact colored pill; the value text is the
  // already-formatted summary (boolean value types resolve to up/down).
  return (
    <span
      className={cn(
        "inline-flex max-w-[8rem] items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs font-medium leading-none",
        STATUS_PILL[status],
      )}
      title={title}
      aria-label={title}
    >
      {render === "boolean" && (
        <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", STATUS_DOT[status])} aria-hidden />
      )}
      <span className="truncate tabular-nums">{summary}</span>
    </span>
  );
};

interface HostLabelStripProps {
  metrics: HostLabelMetric[] | undefined;
  max?: number;
}

export const HostLabelStrip = ({ metrics, max = 3 }: HostLabelStripProps) => {
  const { t } = useTranslation();
  if (!metrics || metrics.length === 0) return null;

  const shown = metrics.slice(0, max);
  const overflow = metrics.slice(max);

  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      {/* `query` is the identity; the index guards a host caching the same query
          twice. The strip is capped (default 3) and read-only. */}
      {shown.map((metric, index) => (
        <MetricLabel key={`${metric.query}:${index}`} metric={metric} />
      ))}
      {overflow.length > 0 && (
        <span
          className="inline-flex items-center rounded-full border border-border bg-muted px-1.5 py-0.5 text-xs font-medium leading-none text-muted-foreground"
          title={overflow
            .map((metric) => {
              const label = metric.label?.trim() || metric.query;
              const summary = metricValueSummary(hostLabelMetricItem(metric), {
                value_type: metric.value_type,
                unit: metric.unit,
              });
              return `${label}: ${summary}`;
            })
            .join("\n")}
          aria-label={t("observability.hostLabel.more", {
            count: overflow.length,
            defaultValue: "{{count}} more",
          })}
        >
          +{overflow.length}
        </span>
      )}
    </span>
  );
};

export default MetricLabel;
