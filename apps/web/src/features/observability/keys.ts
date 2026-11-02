import type { QueryClient } from "@tanstack/react-query";

/**
 * Canonical TanStack Query keys for the observability feature. Centralised so
 * dialogs and screens invalidate the same cache entries, and so dialog files
 * can stay component-only (react-refresh/only-export-components).
 */
export const PRESETS_KEY = ["observability", "presets"] as const;
export const EXPORTER_ASSIGNMENTS_KEY = ["observability", "exporter-assignments"] as const;
export const QUERY_SOURCES_KEY = ["observability", "query-sources"] as const;
export const METRIC_QUERIES_KEY = ["observability", "metric-queries"] as const;
export const METRIC_SETS_KEY = ["observability", "metric-sets"] as const;
export const QUERY_SOURCE_ASSIGNMENTS_KEY = ["observability", "query-source-assignments"] as const;
export const METRIC_SET_ASSIGNMENTS_KEY = ["observability", "metric-set-assignments"] as const;
export const ALERT_SOURCES_KEY = ["observability", "alert-sources"] as const;
export const ALERT_SOURCE_ASSIGNMENTS_KEY = ["observability", "alert-source-assignments"] as const;
export const INSPECTOR_PREVIEW_KEY = ["observability", "connection-target", "inspector-preview"] as const;
export const INSPECTOR_LATEST_KEY = ["observability", "connection-target", "inspector-latest"] as const;

export function invalidateInspectorMetricCaches(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({ queryKey: INSPECTOR_PREVIEW_KEY });
  void queryClient.invalidateQueries({ queryKey: INSPECTOR_LATEST_KEY });
}
