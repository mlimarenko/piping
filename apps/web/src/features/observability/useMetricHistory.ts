import { useQuery, keepPreviousData } from "@tanstack/react-query";

import { getMetricHistory, type MetricHistorySeries } from "@/features/observability/api";
import { parseMetricNumericValue } from "@/features/observability/metricHelpers";
import type { MetricSparklinePoint } from "@/features/observability/MetricWidgetCard";

const DEFAULT_STEP_SECONDS = 60;

export interface UseMetricHistoryOptions {
  rangeSeconds: number;
  step?: number;
  enabled?: boolean;
  refetchInterval?: number | false;
}

export interface UseMetricHistoryResult {
  points: MetricSparklinePoint[];
  isPending: boolean;
  isError: boolean;
  error: Error | null;
}

/**
 * Map the first history series' `[ts_seconds, "value"]` tuples into the
 * `MetricSparklinePoint[]` shape consumed by the sparkline and chart. Defensive
 * because `points` is typed `unknown[]` in the generated schema: non-tuple or
 * non-numeric entries are dropped.
 */
export function mapHistorySeriesToPoints(
  series: MetricHistorySeries[] | undefined,
): MetricSparklinePoint[] {
  const points = series?.[0]?.points ?? [];
  const mapped: MetricSparklinePoint[] = [];
  for (const point of points) {
    if (!Array.isArray(point) || point.length < 2) continue;
    const timestamp = Number(point[0]);
    const value = parseMetricNumericValue(point[1]);
    if (!Number.isFinite(timestamp) || value === null) continue;
    mapped.push({ timestamp, value });
  }
  return mapped;
}

/**
 * Fetch recent metric history for one connection-target metric and project the
 * first series into sparkline points. The query key buckets `now` to the
 * step/range anchor so identical renders within the same bucket share one cache
 * entry instead of refetching on every render.
 */
export function useMetricHistory(
  targetId: string | undefined,
  query: string | undefined,
  { rangeSeconds, step, enabled = true, refetchInterval }: UseMetricHistoryOptions,
): UseMetricHistoryResult {
  const bucket = step ?? DEFAULT_STEP_SECONDS;
  const anchor = Math.floor(Date.now() / 1000 / bucket) * bucket;
  const start = anchor - rangeSeconds;
  const end = anchor;
  const canFetch = enabled && Boolean(targetId) && Boolean(query);

  const historyQuery = useQuery({
    queryKey: [
      "observability",
      "metric-history",
      targetId ?? "",
      query ?? "",
      rangeSeconds,
      step ?? null,
      anchor,
    ],
    queryFn: () => getMetricHistory(targetId ?? "", query ?? "", start, end, step),
    enabled: canFetch,
    placeholderData: keepPreviousData,
    refetchInterval: canFetch ? refetchInterval : false,
    refetchIntervalInBackground: false,
  });

  return {
    points: mapHistorySeriesToPoints(historyQuery.data?.series),
    isPending: historyQuery.isPending,
    isError: historyQuery.isError,
    error: historyQuery.error,
  };
}

// Mini-history adapter: 1-hour range at 60s step refreshed every 5 minutes.
// Used by the host hover card so stat/badge/gauge metrics show a recent
// trend strip alongside the value (sparkline-widget metrics keep their own
// body sparkline).
//
// Rate budget: opening the hover card for a host with N metrics fires N
// independent `/monitoring/history` requests in parallel. The query key
// `(host_id, query, bucketed-by-minute anchor)` lets renders that fall in
// the *same* minute share a cache entry — neighbouring cards on the same
// host within the same minute hit cache instantly — but the anchor rotates
// each minute so a hover re-opened across the boundary refetches. The
// hover-card itself bumps `openDelay` to 400 ms so a quick scan across
// rows does not amplify into N × rows-per-second requests.
const MINI_RANGE_SECONDS = 60 * 60;
const MINI_STEP_SECONDS = 60;
const MINI_REFETCH_MS = 5 * 60 * 1000;

/**
 * Thin wrapper over `useMetricHistory` preset for the hover-card mini
 * sparkline. Returns just the points so callers stay terse; empty/error
 * degrades to `[]` and the card silently drops the strip.
 */
export function useMetricHistorySeries(
  targetId: string | undefined,
  query: string | undefined,
  enabled = true,
): MetricSparklinePoint[] {
  const { points } = useMetricHistory(targetId, query, {
    rangeSeconds: MINI_RANGE_SECONDS,
    step: MINI_STEP_SECONDS,
    enabled,
    refetchInterval: MINI_REFETCH_MS,
  });
  return points;
}
