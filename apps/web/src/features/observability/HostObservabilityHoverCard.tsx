import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { AlertTriangle } from "lucide-react";

import type { HostAlert, HostLabelEntry, HostLabelMetric } from "@/features/observability/api";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/shared/ui-primitives/hover-card";
import MetricWidgetCard from "./MetricWidgetCard";
import { HostLabelStrip } from "./MetricLabel";
import {
  hostLabelMetricDescriptor,
  hostLabelMetricItem,
} from "./hostLabelProjection";
import AlertBadge from "./AlertBadge";
import { alertSeverityStatus } from "./alertSeverity";
import type { MetricCardStatus } from "./metricDisplay";
import { useMetricHistorySeries } from "./useMetricHistory";
import { cn } from "@/shared/lib/utils";

const AVAILABILITY_SEMANTIC = "availability";

const ALERT_DOT: Record<Exclude<MetricCardStatus, "neutral">, string> = {
  normal: "bg-status-positive",
  warning: "bg-status-warning",
  critical: "bg-status-negative",
};

function AlertRow({ alert }: { alert: HostAlert }) {
  const status = alertSeverityStatus(alert.severity);
  const name = alert.name?.trim() || "alert";
  const severity = alert.severity?.trim();
  const summary = alert.summary?.trim();
  return (
    <div className="flex items-start gap-1.5 text-xs leading-snug">
      <span
        className={cn("mt-1 h-1.5 w-1.5 shrink-0 rounded-full", ALERT_DOT[status])}
        aria-hidden
      />
      <div className="min-w-0">
        <div className="flex min-w-0 items-baseline gap-1.5">
          <span className="truncate font-medium text-foreground">{name}</span>
          {severity && (
            <span className="shrink-0 text-xs uppercase tracking-wide text-muted-foreground">
              {severity}
            </span>
          )}
        </div>
        {summary && (
          <p className="line-clamp-2 text-muted-foreground">{summary}</p>
        )}
      </div>
    </div>
  );
}

/**
 * Per-metric row inside the hover card. Pulls a 1-hour mini-history (60s step,
 * cached by `useMetricHistorySeries`'s bucketed-by-minute key) so stat/badge/
 * gauge widgets get a recent trend strip beneath the value; sparkline widgets
 * keep their existing body sparkline (the `MetricWidgetCard` `variant="card"`
 * path threads the `series` through whichever body the widget renders, so
 * there is no double trend). Table/log widgets render no trend strip.
 *
 * Request budget: one `/monitoring/history` request fires per metric per
 * hover-open. The bucketed-by-minute cache key lets neighbouring renders that
 * land in the *same* minute share one cache entry, but the bucket rotates
 * each minute so a hover re-opened across the boundary refetches. The
 * 400 ms `openDelay` on the popover (see below) keeps a quick scan-by from
 * fanning out N requests per host.
 *
 * Hover cards unmount on close, so the query naturally only stays alive while
 * the popover is open. Empty/error silently degrades to the existing
 * value-only card (no error chip in the hover card — quiet).
 *
 * Exported so the test suite can drive the real component end-to-end without
 * having to script Radix HoverCard open events through JSDOM.
 */
export function HoverMetricCard({
  hostId,
  metric,
  stale,
}: {
  hostId: string;
  metric: HostLabelMetric;
  stale: boolean;
}) {
  const { t } = useTranslation();
  const series = useMetricHistorySeries(hostId, metric.query);
  return (
    <MetricWidgetCard
      metric={hostLabelMetricDescriptor(metric)}
      latest={hostLabelMetricItem(metric)}
      stale={stale}
      variant="card"
      series={series.length >= 2 ? series : undefined}
      sparklineLabel={t("observability.metric.recentValues")}
    />
  );
}

interface HostObservabilityHoverCardProps {
  entry: HostLabelEntry | undefined;
  /** The inline cluster (label strip + alert badge) used as the hover anchor. */
  children: ReactNode;
}

/**
 * Hover/focus popover anchored to the inline observability cluster of a
 * Connections host row/tile. Shows the host's list-label metrics as richer
 * `MetricWidgetCard` items plus its active alerts. Radix HoverCard opens only
 * on hover/focus and does not capture click, so the underlying row selection
 * and the existing QuickAccess hover-intent (which fires on the row's
 * `onPointerEnter`) are untouched. Renders the children unwrapped when the host
 * has no cached metrics or alerts, so non-monitored hosts get no empty popover.
 */
const HostObservabilityHoverCard = ({ entry, children }: HostObservabilityHoverCardProps) => {
  const { t } = useTranslation();
  const metrics = entry?.metrics ?? [];
  const displayMetrics = metrics.filter((metric) => metric.semantic !== AVAILABILITY_SEMANTIC);
  const alerts = entry?.alerts ?? [];

  if (metrics.length === 0 && alerts.length === 0) {
    return <>{children}</>;
  }

  // 400 ms openDelay (up from the Radix default ~250 ms) so a quick scan
  // across rows does not fan out one `/monitoring/history` request per
  // metric per row. The mini-history rate budget is documented on
  // `HoverMetricCard` above.
  return (
    <HoverCard openDelay={400} closeDelay={120}>
      <HoverCardTrigger asChild>
        {/*
          A non-interactive span so this never nests interactive content inside
          the surrounding selection button/row (invalid HTML + a phantom tab
          stop). The hover card is pure progressive enhancement on hover; the
          same metric/alert data stays keyboard- and screen-reader-reachable via
          the always-rendered title/aria-label on the inline label strip and
          alert badge.
        */}
        <span className="inline-flex min-w-0 items-center gap-1">{children}</span>
      </HoverCardTrigger>
      <HoverCardContent
        align="start"
        className="w-80 space-y-2"
        // Hover cards open on row hover; clicking inside must not bubble up to
        // the row's selection handler.
        onClick={(event) => event.stopPropagation()}
      >
        {alerts.length > 0 && (
          <div className="space-y-1.5">
            <div className="flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
              <AlertTriangle className="h-3 w-3 text-status-warning" aria-hidden />
              <span>{t("observability.hover.alerts", { defaultValue: "Active alerts" })}</span>
            </div>
            <div className="space-y-1.5">
              {/* Alerts share an alertname across instances and carry no stable
                  id, so the index disambiguates; the list is short and read-only. */}
              {alerts.map((alert, index) => (
                <AlertRow key={`${alert.name}:${index}`} alert={alert} />
              ))}
            </div>
          </div>
        )}

        {displayMetrics.length > 0 && entry?.host_id && (
          <div className="space-y-1.5">
            <div className="text-xs font-medium text-muted-foreground">
              {t("observability.hover.metrics", { defaultValue: "Metrics" })}
            </div>
            <div className="space-y-1.5">
              {/* `query` is the natural identity; the index guards the rare case
                  of a host caching the same query twice. Short, read-only list.
                  Host id is hoisted into the conditional above so the strict
                  contract — `useMetricHistorySeries` needs a real id — is
                  visible at the render site instead of silently degrading via
                  an empty-string fallback. */}
              {displayMetrics.map((metric, index) => (
                <HoverMetricCard
                  key={`${metric.query}:${index}`}
                  hostId={entry.host_id}
                  metric={metric}
                  stale={entry.stale ?? false}
                />
              ))}
            </div>
          </div>
        )}
      </HoverCardContent>
    </HoverCard>
  );
};

interface HostObservabilityClusterProps {
  entry: HostLabelEntry | undefined;
  /** Forwarded to `HostLabelStrip` to cap inline labels per layout density. */
  maxLabels?: number;
  /**
   * Additional classes for the inline wrapper. Connections rows pass
   * `group-hover:opacity-0` so the absolute-positioned QuickAccessButtons
   * overlay does not visually cover the metrics on hover.
   */
  className?: string;
}

/**
 * The inline observability cluster used in every Connections host row/tile/cell:
 * the compact metric-label strip plus the alert badge, wrapped in the hover
 * card so the richer metric/alert popover is available on hover or focus. One
 * composition shared across TreeView, CardsView and the table name column so
 * the three sites stay identical and read the same threaded `byHost` entry.
 */
export const HostObservabilityCluster = ({
  entry,
  maxLabels,
  className,
}: HostObservabilityClusterProps) => {
  if (!entry || ((entry.metrics?.length ?? 0) === 0 && (entry.alerts?.length ?? 0) === 0)) {
    return null;
  }
  return (
    <span className={cn("inline-flex items-center gap-1 transition-opacity", className)}>
      <HostObservabilityHoverCard entry={entry}>
        <HostLabelStrip
          metrics={entry.metrics?.filter((metric) => metric.semantic !== AVAILABILITY_SEMANTIC)}
          max={maxLabels}
        />
        <AlertBadge
          alerts={entry.alerts}
          metrics={entry.metrics?.filter((metric) => metric.semantic !== AVAILABILITY_SEMANTIC)}
          stale={entry.stale}
        />
      </HostObservabilityHoverCard>
    </span>
  );
};
