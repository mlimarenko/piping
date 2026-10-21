import type { ReactNode } from "react";
import {
  Activity,
  AlertTriangle,
  BadgeCheck,
  FileText,
  Gauge,
  Loader2,
  Sparkles,
  Table2,
} from "lucide-react";

import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import {
  metricHasValue,
  metricValueSummary,
  parseMetricNumericValue,
  rawMetricValue,
  sparklineValueScale,
} from "@/features/observability/metricHelpers";
import {
  metricCardStatus,
  type MetricCardStatus,
} from "@/features/observability/metricDisplay";
import { Badge } from "@/shared/ui-primitives/badge";
import { cn } from "@/shared/lib/utils";

interface MetricWidgetCardProps {
  metric: MonitoringMetricDescriptor;
  latest?: LatestMetricItem;
  loading?: boolean;
  stale?: boolean;
  series?: MetricSparklinePoint[];
  sparklineLabel?: string;
  /**
   * `card` (default): bordered widget with rail, icon, optional badge —
   * the rich popover surface used by `HostObservabilityHoverCard`.
   * `row`: dense single-line row tuned for the inspector panel — borderless,
   * no gradient, no status pill; label left, value right, status carried by a
   * 2px hairline rail + the small `AlertTriangle` glyph for warning/critical.
   * Empty values render as a quiet em-dash, matching the surrounding InfoRow
   * pattern (Информация / Детали / Контекст конфигурации).
   */
  variant?: "card" | "row";
  /**
   * Optional trailing affordance for the `row` variant — for example the
   * per-host override pencil icon. Rendered only on hover so the inspector
   * row stays quiet at rest.
   */
  rowAction?: ReactNode;
}

type JsonRecord = Record<string, unknown>;
type WidgetKind = "stat" | "badge" | "gauge" | "sparkline" | "table" | "log";

export interface MetricSparklinePoint {
  timestamp: number;
  value: number;
}

const STATUS_STYLE: Record<
  MetricCardStatus,
  {
    card: string;
    rail: string;
    icon: string;
    value: string;
    chip: string;
    gauge: string;
  }
> = {
  neutral: {
    card: "border-border bg-card",
    rail: "bg-border/50",
    icon: "text-muted-foreground",
    value: "text-foreground",
    chip: "border-border bg-muted text-muted-foreground",
    gauge: "bg-muted-foreground/45",
  },
  normal: {
    card: "border-border bg-card",
    rail: "bg-status-positive",
    icon: "text-status-positive",
    value: "text-status-positive",
    chip: "border-status-positive/35 bg-card text-status-positive",
    gauge: "bg-status-positive",
  },
  warning: {
    card: "border-border bg-card",
    rail: "bg-status-warning",
    icon: "text-status-warning",
    value: "text-status-warning",
    chip: "border-status-warning/45 bg-card text-status-warning",
    gauge: "bg-status-warning",
  },
  critical: {
    card: "border-border bg-card",
    rail: "bg-status-negative",
    icon: "text-status-negative",
    value: "text-status-negative",
    chip: "border-status-negative/45 bg-card text-status-negative",
    gauge: "bg-status-negative",
  },
};

const STATUS_LABEL: Record<MetricCardStatus, string> = {
  neutral: "",
  normal: "OK",
  warning: "Warning",
  critical: "Critical",
};

function asRecord(value: unknown): JsonRecord | null {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as JsonRecord)
    : null;
}

function optionNumber(value: unknown): number | null {
  return parseMetricNumericValue(value);
}

function displayOptions(metric: MonitoringMetricDescriptor): JsonRecord | null {
  return asRecord(asRecord(metric.options)?.display);
}

function normalizedToken(value: unknown): string {
  return typeof value === "string" ? value.trim().toLowerCase() : "";
}

function metricDescription(metric: MonitoringMetricDescriptor, title: string): string | null {
  const description = typeof metric.description === "string" ? metric.description.trim() : "";
  if (!description || description === title) return null;
  return description;
}

function metricTitle(metric: MonitoringMetricDescriptor, latest?: LatestMetricItem): string {
  return (
    [metric.label, latest?.label, metric.name, latest?.name]
      .map((value) => (typeof value === "string" ? value.trim() : ""))
      .find(Boolean) ?? "Metric"
  );
}

function displayValue(metric: MonitoringMetricDescriptor, latest?: LatestMetricItem): string {
  if (!latest) return "—";
  return metricValueSummary(latest, metric);
}

function metricNumericValue(latest?: LatestMetricItem): number | null {
  return latest ? parseMetricNumericValue(rawMetricValue(latest)) : null;
}

function widgetKind(metric: MonitoringMetricDescriptor, latest?: LatestMetricItem): WidgetKind {
  const candidate = normalizedToken(metric.widget_type || latest?.widget_type);
  if (["stat", "badge", "gauge", "sparkline", "table", "log"].includes(candidate)) {
    return candidate as WidgetKind;
  }
  return "stat";
}

function isPercentMetric(metric: MonitoringMetricDescriptor, latest?: LatestMetricItem): boolean {
  return (
    normalizedToken(metric.value_type || latest?.value_type) === "percent" ||
    normalizedToken(metric.unit) === "%"
  );
}

function gaugeRange(metric: MonitoringMetricDescriptor, latest?: LatestMetricItem) {
  const options = displayOptions(metric);
  const range = asRecord(options?.range);
  const min = optionNumber(options?.min ?? range?.min) ?? 0;
  const max = optionNumber(options?.max ?? range?.max) ?? (isPercentMetric(metric, latest) ? 100 : null);
  if (max === null || max <= min) return null;
  return { min, max };
}

function sparklinePointLimit(metric: MonitoringMetricDescriptor): number {
  const options = displayOptions(metric);
  const sparkline = asRecord(options?.sparkline);
  const configured = optionNumber(sparkline?.points ?? sparkline?.samples);
  if (configured === null) return 24;
  return Math.max(2, Math.min(60, Math.floor(configured)));
}

function sparklinePath(points: MetricSparklinePoint[]): string | null {
  if (points.length < 2) return null;
  const normalize = sparklineValueScale(points.map((point) => point.value));
  return points
    .map((point, index) => {
      const x = (index / (points.length - 1)) * 100;
      const y = 22 - normalize(point.value) * 18;
      return `${index === 0 ? "M" : "L"} ${x.toFixed(2)} ${y.toFixed(2)}`;
    })
    .join(" ");
}

function metricRows(latest?: LatestMetricItem): Array<{ labels: Record<string, string>; value: string }> {
  const body = latest?.value as
    | {
        data?: {
          result?: Array<{ metric?: Record<string, string>; value?: [number, string] }>;
        };
      }
    | undefined;
  return (body?.data?.result ?? []).slice(0, 4).map((row) => ({
    labels: row.metric ?? {},
    value: row.value?.[1] ?? "—",
  }));
}

function labelSummary(labels: Record<string, string>): string {
  const entries = Object.entries(labels).filter(([key]) => key !== "__name__");
  if (entries.length === 0) return "value";
  return entries
    .slice(0, 3)
    .map(([key, value]) => `${key}=${value}`)
    .join(" · ");
}

function MetricStatusIcon({
  status,
  widget,
  className,
}: {
  status: MetricCardStatus;
  widget: WidgetKind;
  className?: string;
}) {
  if (status === "critical" || status === "warning") return <AlertTriangle className={className} />;
  if (widget === "gauge") return <Gauge className={className} />;
  if (widget === "table") return <Table2 className={className} />;
  if (widget === "log") return <FileText className={className} />;
  if (status === "normal") return <BadgeCheck className={className} />;
  if (widget === "sparkline") return <Sparkles className={className} />;
  return <Activity className={className} />;
}

function StatValue({ value, className }: { value: string; className?: string }) {
  return (
    <span
      className={cn("block truncate text-right text-sm font-semibold leading-none tabular-nums", className)}
      title={value}
    >
      {value}
    </span>
  );
}

function GaugeBody({
  metric,
  latest,
  value,
  status,
}: {
  metric: MonitoringMetricDescriptor;
  latest?: LatestMetricItem;
  value: string;
  status: MetricCardStatus;
}) {
  const style = STATUS_STYLE[status];
  const numeric = metricNumericValue(latest);
  const range = gaugeRange(metric, latest);
  if (numeric === null || !range) {
    return <StatValue value={value} className={style.value} />;
  }
  const percent = Math.max(0, Math.min(100, ((numeric - range.min) / (range.max - range.min)) * 100));
  return (
    <div className="min-w-[6.5rem] space-y-1 text-right">
      <StatValue value={value} className={style.value} />
      <div className="h-1 overflow-hidden rounded-full bg-muted" aria-hidden>
        <div className={cn("h-full rounded-full transition-all", style.gauge)} style={{ width: `${percent}%` }} />
      </div>
    </div>
  );
}

function SparklineBody({
  metric,
  value,
  status,
  series,
  sparklineLabel,
}: {
  metric: MonitoringMetricDescriptor;
  value: string;
  status: MetricCardStatus;
  series?: MetricSparklinePoint[];
  sparklineLabel: string;
}) {
  const style = STATUS_STYLE[status];
  const points = (series ?? [])
    .filter((point) => Number.isFinite(point.value))
    .slice(-sparklinePointLimit(metric));
  const path = sparklinePath(points);

  return (
    <div className="min-w-[7rem] space-y-1 text-right">
      <StatValue value={value} className={style.value} />
      <svg
        className={cn("h-6 w-full overflow-visible", style.value)}
        viewBox="0 0 100 24"
        preserveAspectRatio="none"
        role="img"
        aria-label={sparklineLabel}
      >
        <path
          d="M 0 18 L 100 18"
          className="stroke-muted-foreground/20"
          strokeWidth="1.5"
          vectorEffect="non-scaling-stroke"
          fill="none"
        />
        {path ? (
          <path
            data-testid="metric-sparkline-line"
            d={path}
            className="fill-none stroke-current"
            strokeWidth="1.7"
            strokeLinecap="round"
            strokeLinejoin="round"
            vectorEffect="non-scaling-stroke"
          />
        ) : (
          <path
            data-testid="metric-sparkline-line"
            d="M 0 18 L 100 18"
            className="fill-none stroke-current opacity-45"
            strokeWidth="1.5"
            strokeLinecap="round"
            vectorEffect="non-scaling-stroke"
          />
        )}
      </svg>
    </div>
  );
}

function TableBody({ latest, status }: { latest?: LatestMetricItem; status: MetricCardStatus }) {
  const rows = metricRows(latest);
  if (rows.length === 0) return <StatValue value={displayValue({ unit: null } as MonitoringMetricDescriptor, latest)} className={STATUS_STYLE[status].value} />;
  return (
    <div className="min-w-[10rem] max-w-[14rem] space-y-1 text-right">
      {rows.map((row, index) => (
        <div key={`${labelSummary(row.labels)}:${index}`} className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 text-xs">
          <span className="truncate text-muted-foreground" title={labelSummary(row.labels)}>
            {labelSummary(row.labels)}
          </span>
          <span className={cn("font-semibold tabular-nums", STATUS_STYLE[status].value)}>{row.value}</span>
        </div>
      ))}
    </div>
  );
}

function InlineSparkline({
  series,
  status,
  sparklineLabel,
  metric,
}: {
  series?: MetricSparklinePoint[];
  status: MetricCardStatus;
  sparklineLabel: string;
  metric: MonitoringMetricDescriptor;
}) {
  const style = STATUS_STYLE[status];
  const points = (series ?? [])
    .filter((point) => Number.isFinite(point.value))
    .slice(-sparklinePointLimit(metric));
  const path = sparklinePath(points);
  return (
    <svg
      className={cn("h-3 w-12 shrink-0 overflow-visible opacity-80", style.value)}
      viewBox="0 0 100 24"
      preserveAspectRatio="none"
      role="img"
      aria-label={sparklineLabel}
    >
      <path
        d="M 0 18 L 100 18"
        className="stroke-muted-foreground/15"
        strokeWidth="1.5"
        vectorEffect="non-scaling-stroke"
        fill="none"
      />
      {path ? (
        <path
          data-testid="metric-sparkline-line"
          d={path}
          className="fill-none stroke-current"
          strokeWidth="1.7"
          strokeLinecap="round"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
      ) : (
        <path
          data-testid="metric-sparkline-line"
          d="M 0 18 L 100 18"
          className="fill-none stroke-current opacity-45"
          strokeWidth="1.5"
          strokeLinecap="round"
          vectorEffect="non-scaling-stroke"
        />
      )}
    </svg>
  );
}

/**
 * Compact full-width trend strip rendered beneath the value pill of a
 * card-variant `MetricWidgetCard` when the caller threads in mini-history.
 * Reused only by the hover card today — every metric there gets a last-hour
 * mini sparkline regardless of `widget_type` so the operator sees the trend at
 * a glance. Gated on ≥2 finite points so cards without history backfill stay
 * visually unchanged (silent degrade).
 */
function MiniTrendStrip({
  series,
  status,
  sparklineLabel,
  metric,
}: {
  series: MetricSparklinePoint[];
  status: MetricCardStatus;
  sparklineLabel: string;
  metric: MonitoringMetricDescriptor;
}) {
  const style = STATUS_STYLE[status];
  const points = series
    .filter((point) => Number.isFinite(point.value))
    .slice(-sparklinePointLimit(metric));
  const path = sparklinePath(points);
  if (!path) return null;
  return (
    <svg
      className={cn("mt-1 h-4 w-full overflow-visible opacity-80", style.value)}
      viewBox="0 0 100 24"
      preserveAspectRatio="none"
      role="img"
      aria-label={sparklineLabel}
      data-testid="metric-mini-trend"
    >
      <path
        d="M 0 18 L 100 18"
        className="stroke-muted-foreground/15"
        strokeWidth="1.5"
        vectorEffect="non-scaling-stroke"
        fill="none"
      />
      <path
        data-testid="metric-mini-trend-line"
        d={path}
        className="fill-none stroke-current"
        strokeWidth="1.7"
        strokeLinecap="round"
        strokeLinejoin="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}

function InlineGaugeTrack({
  metric,
  latest,
  status,
}: {
  metric: MonitoringMetricDescriptor;
  latest?: LatestMetricItem;
  status: MetricCardStatus;
}) {
  const style = STATUS_STYLE[status];
  const numeric = metricNumericValue(latest);
  const range = gaugeRange(metric, latest);
  if (numeric === null || !range) return null;
  const percent = Math.max(0, Math.min(100, ((numeric - range.min) / (range.max - range.min)) * 100));
  return (
    <div className="h-1 w-10 shrink-0 overflow-hidden rounded-full bg-muted" aria-hidden>
      <div className={cn("h-full rounded-full transition-all", style.gauge)} style={{ width: `${percent}%` }} />
    </div>
  );
}

/**
 * Dense inspector-row rendering: matches the calm InfoRow surface used by the
 * surrounding `Информация / Детали / Контекст конфигурации` sections in
 * `TreeNodeInspector` / `ConnectionInspector`. The row is borderless, sits on
 * the section's existing surface (no per-row tint), and conveys status with a
 * 2px left hairline rail and a small `AlertTriangle` glyph for warning /
 * critical (shape + color, not color alone). Empty values render as `—`.
 */
function MetricInspectorRow({
  title,
  description,
  status,
  isError,
  loading,
  hasValue,
  widget,
  value,
  metric,
  latest,
  series,
  sparklineLabel,
  rowAction,
}: {
  title: string;
  description: string | null;
  status: MetricCardStatus;
  isError: boolean;
  loading: boolean;
  hasValue: boolean;
  widget: WidgetKind;
  value: string;
  metric: MonitoringMetricDescriptor;
  latest?: LatestMetricItem;
  series?: MetricSparklinePoint[];
  sparklineLabel: string;
  rowAction?: ReactNode;
}) {
  const effectiveStatus: MetricCardStatus = isError ? "critical" : status;
  const effectiveStyle = STATUS_STYLE[effectiveStatus];
  const showGlyph = effectiveStatus === "warning" || effectiveStatus === "critical";
  // No data / cache miss / explicit error → quiet em-dash, matching the
  // surrounding InfoRow handling of empty fields. No "No data" pill.
  const showEmDash = !loading && (!hasValue || isError);
  const displayed = showEmDash ? "—" : value;

  return (
    <div
      className="group relative flex items-center gap-2 py-1 pl-2.5 pr-1 text-xs"
      aria-label={`${title}: ${displayed}`}
      title={description ?? undefined}
    >
      {/* 2px hairline rail carries the status color; visually quiet, no card border */}
      <span
        className={cn(
          "absolute inset-y-1 left-0 w-[2px] rounded-full",
          status === "neutral" && !isError ? "bg-transparent" : effectiveStyle.rail,
        )}
        aria-hidden
      />
      <div className="flex min-w-0 flex-1 items-center gap-1.5">
        {showGlyph && (
          <AlertTriangle
            aria-hidden
            className={cn("h-3 w-3 shrink-0", effectiveStyle.icon)}
          />
        )}
        <span className="min-w-0 truncate text-muted-foreground">{title}</span>
      </div>
      <div className="flex shrink-0 items-center gap-1.5">
        {widget === "sparkline" && !showEmDash && (
          <InlineSparkline
            series={series}
            status={effectiveStatus}
            sparklineLabel={sparklineLabel}
            metric={metric}
          />
        )}
        {widget === "gauge" && !showEmDash && (
          <InlineGaugeTrack metric={metric} latest={latest} status={effectiveStatus} />
        )}
        {loading ? (
          <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />
        ) : (
          <span
            className="max-w-[10rem] truncate text-right tabular-nums text-foreground"
            title={displayed}
          >
            {displayed}
          </span>
        )}
        {rowAction ? (
          // Keep the override action readable without hover, including touch.
          <span className="text-muted-foreground">
            {rowAction}
          </span>
        ) : null}
      </div>
    </div>
  );
}

const MetricWidgetCard = ({
  metric,
  latest,
  loading = false,
  stale = false,
  series,
  sparklineLabel = "Recent values",
  variant = "card",
  rowAction,
}: MetricWidgetCardProps) => {
  const hasValue = metricHasValue(latest);
  const rawSummary = latest ? metricValueSummary(latest, metric) : "—";
  const status = stale ? "neutral" : metricCardStatus(metric, latest, rawSummary);
  const style = STATUS_STYLE[status];
  const widget = widgetKind(metric, latest);
  const value = rawSummary;
  const title = metricTitle(metric, latest);
  const description = metricDescription(metric, title);
  const isError = Boolean(latest?.error);

  if (variant === "row") {
    return (
      <MetricInspectorRow
        title={title}
        description={description}
        status={status}
        isError={isError}
        loading={loading}
        hasValue={hasValue}
        widget={widget}
        value={value}
        metric={metric}
        latest={latest}
        series={series}
        sparklineLabel={sparklineLabel}
        rowAction={rowAction}
      />
    );
  }
  // `stale` is an internal cache-miss signal; treating it as a status badge
  // makes brief between-poll gaps look like an outage. Show a real "Error" /
  // "No data" pill only when the metric itself lacks a value.
  const chipLabel = isError
    ? "Error"
    : !loading && !hasValue
      ? "No data"
      : status === "neutral"
        ? null
        : STATUS_LABEL[status];
  const isLog = widget === "log" || metric.value_type === "log" || latest?.value_type === "log";
  // A `series` thread-through draws a compact trend strip beneath the value
  // for stat/badge/gauge widgets (sparkline/table/log already have their own
  // body rendering). Caller-gated on ≥2 finite points; the strip degrades to
  // null silently otherwise so cards without history stay visually unchanged.
  const showMiniTrend =
    variant === "card"
      && !loading
      && hasValue
      && !isError
      && (series?.length ?? 0) >= 2
      && (widget === "stat" || widget === "badge" || widget === "gauge");

  return (
    <div
      className={cn(
        "relative grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-2 gap-y-1 overflow-hidden rounded-md border px-2.5 py-2 pl-3 transition-colors",
        style.card,
      )}
      aria-label={`${title}: ${value}`}
    >
      <span className={cn("absolute inset-y-0 left-0 w-[3px]", style.rail)} aria-hidden />
      <div className="min-w-0 space-y-0.5">
        <div className="flex min-w-0 items-center gap-1.5">
          <MetricStatusIcon
            status={status}
            widget={widget}
            className={cn("h-3 w-3 shrink-0", isError ? "text-status-negative" : style.icon)}
          />
          <p className="min-w-0 truncate text-xs font-medium leading-snug text-foreground">
            {title}
          </p>
          {chipLabel && (
            <Badge
              variant="outline"
              className={cn("rounded-md px-1.5 text-xs uppercase tracking-wide", style.chip)}
            >
              {chipLabel}
            </Badge>
          )}
        </div>
        {description && (
          <p className="line-clamp-1 min-w-0 text-xs leading-snug text-muted-foreground">
            {description}
          </p>
        )}
      </div>

      <div className="min-w-[3.25rem] text-right">
        {loading ? (
          <Loader2 className="ml-auto h-3.5 w-3.5 animate-spin text-muted-foreground" />
        ) : isLog ? (
          <span
            className={cn(
              "block max-w-[12rem] whitespace-pre-wrap break-words text-right text-xs font-medium leading-snug",
              isError ? "text-status-negative" : style.value,
            )}
            title={value}
          >
            {value}
          </span>
        ) : widget === "badge" ? (
          <Badge variant="outline" className={cn("max-w-[8rem] truncate rounded-full px-2 py-0.5 text-xs font-semibold", isError ? STATUS_STYLE.critical.chip : style.chip)}>
            {value}
          </Badge>
        ) : widget === "gauge" ? (
          <GaugeBody metric={metric} latest={latest} value={value} status={isError ? "critical" : status} />
        ) : widget === "sparkline" ? (
          <SparklineBody
            metric={metric}
            value={value}
            status={isError ? "critical" : status}
            series={series}
            sparklineLabel={sparklineLabel}
          />
        ) : widget === "table" ? (
          <TableBody latest={latest} status={isError ? "critical" : status} />
        ) : (
          <StatValue value={value} className={isError ? "text-status-negative" : style.value} />
        )}
      </div>

      {showMiniTrend && series && (
        <div className="col-span-2 -mt-0.5">
          <MiniTrendStrip
            metric={metric}
            status={isError ? "critical" : status}
            sparklineLabel={sparklineLabel}
            series={series}
          />
        </div>
      )}
    </div>
  );
};

export default MetricWidgetCard;
