import { useTranslation } from "react-i18next";

import type { HostLabelEntry } from "@/features/observability/api";
import {
  findAvailabilityMetric,
  resolveHostAvailabilityState,
  STATE_DOT,
  STATE_PILL,
} from "@/features/observability/hostAvailability";
import { cn } from "@/shared/lib/utils";

interface HostAvailabilityIndicatorProps {
  entry: HostLabelEntry | undefined;
  className?: string;
}

export const HostAvailabilityIndicator = ({ entry, className }: HostAvailabilityIndicatorProps) => {
  const { t } = useTranslation();
  const metric = findAvailabilityMetric(entry);
  const state = resolveHostAvailabilityState(entry);
  const stateLabel = t(`observability.availability.${state}`);
  const metricLabel = metric?.label?.trim() || metric?.query;
  const title = metricLabel ? `${stateLabel}: ${metricLabel}` : stateLabel;

  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center gap-1 rounded-full border px-1.5 py-0.5 text-xs font-medium leading-none",
        STATE_PILL[state],
        className,
      )}
      title={title}
      aria-label={title}
    >
      <span className={cn("h-1.5 w-1.5 rounded-full", STATE_DOT[state])} aria-hidden />
      <span>{stateLabel}</span>
    </span>
  );
};
