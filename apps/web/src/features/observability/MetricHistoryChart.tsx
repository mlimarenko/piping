import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Loader2 } from "lucide-react";

import type {
  LatestMetricItem,
  MonitoringMetricDescriptor,
} from "@/features/observability/api";
import { metricCardStatus, type MetricCardStatus } from "@/features/observability/metricDisplay";
import {
  metricValueSummary,
  sparklineValueScale,
} from "@/features/observability/metricHelpers";
import type { MetricSparklinePoint } from "@/features/observability/MetricWidgetCard";
import { useMetricHistory } from "@/features/observability/useMetricHistory";
import { cn } from "@/shared/lib/utils";

interface RangePreset {
  id: string;
  rangeSeconds: number;
  step: number;
  labelKey: string;
}

const DEFAULT_PRESET: RangePreset = {
  id: "1h",
  rangeSeconds: 60 * 60,
  step: 60,
  labelKey: "observability.history.ranges.1h",
};

const RANGE_PRESETS: RangePreset[] = [
  DEFAULT_PRESET,
  { id: "6h", rangeSeconds: 6 * 60 * 60, step: 5 * 60, labelKey: "observability.history.ranges.6h" },
  { id: "24h", rangeSeconds: 24 * 60 * 60, step: 15 * 60, labelKey: "observability.history.ranges.24h" },
];

const VIEW_WIDTH = 320;
const VIEW_HEIGHT = 96;
const PAD_X = 4;
const PAD_TOP = 6;
const PAD_BOTTOM = 6;

const STATUS_LINE: Record<MetricCardStatus, string> = {
  neutral: "text-muted-foreground",
  normal: "text-status-positive",
  warning: "text-status-warning",
  critical: "text-status-negative",
};

interface PlottedPoint extends MetricSparklinePoint {
  x: number;
  y: number;
}

function plotPoints(points: MetricSparklinePoint[]): PlottedPoint[] {
  if (points.length === 0) return [];
  const normalize = sparklineValueScale(points.map((point) => point.value));
  const innerWidth = VIEW_WIDTH - PAD_X * 2;
  const innerHeight = VIEW_HEIGHT - PAD_TOP - PAD_BOTTOM;
  const denominator = points.length > 1 ? points.length - 1 : 1;
  return points.map((point, index) => {
    const x = PAD_X + (index / denominator) * innerWidth;
    const y = PAD_TOP + (1 - normalize(point.value)) * innerHeight;
    return { ...point, x, y };
  });
}

function linePath(plotted: PlottedPoint[]): string | null {
  if (plotted.length < 2) return null;
  return plotted
    .map((point, index) => `${index === 0 ? "M" : "L"} ${point.x.toFixed(2)} ${point.y.toFixed(2)}`)
    .join(" ");
}

interface MetricHistoryChartProps {
  targetId: string;
  metric: MonitoringMetricDescriptor;
}

const MetricHistoryChart = ({ targetId, metric }: MetricHistoryChartProps) => {
  const { t } = useTranslation();
  const [presetId, setPresetId] = useState(DEFAULT_PRESET.id);
  const preset = RANGE_PRESETS.find((entry) => entry.id === presetId) ?? DEFAULT_PRESET;
  const [hoverIndex, setHoverIndex] = useState<number | null>(null);

  const history = useMetricHistory(targetId, metric.query, {
    rangeSeconds: preset.rangeSeconds,
    step: preset.step,
    enabled: true,
    refetchInterval: 60_000,
  });

  const plotted = useMemo(() => plotPoints(history.points), [history.points]);
  const path = linePath(plotted);
  const lastPoint = history.points.at(-1);
  const status: MetricCardStatus = lastPoint
    ? metricCardStatus(metric, syntheticLatest(metric, lastPoint.value), formatPointValue(metric, lastPoint.value))
    : "neutral";
  const lineClass = STATUS_LINE[status];

  const hoverPoint =
    hoverIndex !== null && hoverIndex >= 0 && hoverIndex < plotted.length
      ? plotted[hoverIndex]
      : null;

  return (
    <div className="grid gap-2" data-testid="metric-history-chart">
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-1.5 rounded-md border border-border p-0.5">
          {RANGE_PRESETS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              onClick={() => setPresetId(entry.id)}
              className={cn(
                "h-8 rounded-md px-2 text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring",
                entry.id === presetId
                  ? "bg-accent text-primary"
                  : "text-muted-foreground hover:text-foreground",
              )}
              aria-pressed={entry.id === presetId}
            >
              {t(entry.labelKey)}
            </button>
          ))}
        </div>
        <span className="min-h-4 text-xs tabular-nums text-muted-foreground" aria-live="polite">
          {hoverPoint
            ? `${new Date(hoverPoint.timestamp * 1000).toLocaleString()} · ${formatPointValue(metric, hoverPoint.value)}`
            : lastPoint
              ? formatPointValue(metric, lastPoint.value)
              : ""}
        </span>
      </div>

      <div className="relative h-24 overflow-hidden rounded-md border border-border bg-background">
        {history.isPending ? (
          <div className="flex h-full items-center justify-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            {t("loading")}
          </div>
        ) : history.isError ? (
          <div className="flex h-full items-center justify-center gap-2 px-3 text-center text-xs text-muted-foreground">
            <AlertTriangle className="h-3.5 w-3.5 shrink-0 text-status-warning" />
            <span className="truncate">{history.error?.message ?? t("observability.history.error")}</span>
          </div>
        ) : plotted.length === 0 ? (
          <div className="flex h-full items-center justify-center text-xs text-muted-foreground">
            {t("observability.history.noData")}
          </div>
        ) : (
          <svg
            className={cn("h-full w-full", lineClass)}
            viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={t("observability.history.chartLabel", {
              metric: metric.label ?? metric.name,
            })}
            onMouseLeave={() => setHoverIndex(null)}
            onMouseMove={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              if (rect.width === 0 || plotted.length === 0) return;
              const ratio = (event.clientX - rect.left) / rect.width;
              const index = Math.round(ratio * (plotted.length - 1));
              setHoverIndex(Math.max(0, Math.min(plotted.length - 1, index)));
            }}
          >
            <line
              x1={PAD_X}
              y1={VIEW_HEIGHT - PAD_BOTTOM}
              x2={VIEW_WIDTH - PAD_X}
              y2={VIEW_HEIGHT - PAD_BOTTOM}
              className="stroke-border/40"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
            {path && (
              <path
                data-testid="metric-history-line"
                d={path}
                className="fill-none stroke-current"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
                vectorEffect="non-scaling-stroke"
              />
            )}
            {hoverPoint && (
              <g>
                <line
                  x1={hoverPoint.x}
                  y1={PAD_TOP}
                  x2={hoverPoint.x}
                  y2={VIEW_HEIGHT - PAD_BOTTOM}
                  className="stroke-border/60"
                  strokeWidth="1"
                  strokeDasharray="2 2"
                  vectorEffect="non-scaling-stroke"
                />
                <circle cx={hoverPoint.x} cy={hoverPoint.y} r="2.5" className="fill-current" />
              </g>
            )}
          </svg>
        )}
      </div>
    </div>
  );
};

function syntheticLatest(metric: MonitoringMetricDescriptor, value: number): LatestMetricItem {
  return {
    query: metric.query,
    status: "success",
    value: value.toString(),
    value_type: metric.value_type ?? null,
    unit: metric.unit ?? null,
  };
}

function formatPointValue(metric: MonitoringMetricDescriptor, value: number): string {
  return metricValueSummary(syntheticLatest(metric, value), metric);
}

export default MetricHistoryChart;
