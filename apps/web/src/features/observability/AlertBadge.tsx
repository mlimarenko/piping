import { useTranslation } from "react-i18next";
import { AlertTriangle } from "lucide-react";

import type { HostAlert, HostLabelMetric } from "@/features/observability/api";
import {
  alertLine,
  rollupHostAlerts,
  type AlertStatus,
} from "./alertSeverity";
import { cn } from "@/shared/lib/utils";

// Same CDS status tokens StatusChip / MetricLabel / MetricWidgetCard use.
const STATUS_PILL: Record<AlertStatus, string> = {
  normal: "border-status-positive/35 bg-card text-status-positive",
  warning: "border-status-warning/45 bg-card text-status-warning",
  critical: "border-status-negative/45 bg-card text-status-negative",
};

interface AlertBadgeProps {
  alerts: HostAlert[] | undefined;
  metrics?: HostLabelMetric[];
  stale?: boolean;
  className?: string;
}

/**
 * Compact per-host alert indicator for the Connections list. Renders nothing
 * when there are no firing alerts and the metric rollup is healthy / no-data.
 * Shows the alert count plus a worst-severity colored pill; when only a metric
 * threshold breach is present (no firing alerts), it shows a severity dot
 * without a count instead of a misleading "0".
 */
const AlertBadge = ({ alerts, metrics, stale = false, className }: AlertBadgeProps) => {
  const { t } = useTranslation();
  const { count, status } = rollupHostAlerts(alerts, metrics, stale);

  if (status === null) return null;

  const list = alerts ?? [];
  const title =
    list.length > 0
      ? list.map(alertLine).join("\n")
      : t("observability.alert.thresholdBreach", { defaultValue: "Metric threshold breached" });

  const label =
    count > 0
      ? t("observability.alert.count", { count, defaultValue: "{{count}} alerts" })
      : t("observability.alert.thresholdBreach", { defaultValue: "Metric threshold breached" });

  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs font-medium leading-none",
        STATUS_PILL[status],
        className,
      )}
      title={title}
      aria-label={label}
    >
      <AlertTriangle className="h-2.5 w-2.5 shrink-0" aria-hidden />
      {count > 0 && <span className="tabular-nums">{count}</span>}
    </span>
  );
};

export default AlertBadge;
