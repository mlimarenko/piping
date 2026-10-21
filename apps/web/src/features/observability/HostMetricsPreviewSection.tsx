import { useCallback, useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  AlertTriangle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  CircleHelp,
  Loader2,
  Pencil,
} from "lucide-react";

import {
  getConnectionTargetObservability,
  getLatestMetrics,
  type LatestMetricItem,
  type MonitoringMetricDescriptor,
} from "@/features/observability/api";
import { Button } from "@/shared/ui-primitives/button";
import HostMetricOverrideDrawer from "./HostMetricOverrideDrawer";
import { usePermissions } from "@/shared/auth/permissionsContext";
import {
  summarizeHostObservability,
  type HostObservabilityState,
  type HostObservabilitySummary,
} from "@/features/observability/hostObservabilitySummary";
import {
  filterMetricsWithResolvedSources,
  groupMetricsByCategory,
  metricHasValue,
  metricKey,
  parseMetricNumericValue,
  rawMetricValue,
} from "@/features/observability/metricHelpers";
import MetricWidgetCard, {
  type MetricSparklinePoint,
} from "@/features/observability/MetricWidgetCard";
import { useMetricHistory } from "@/features/observability/useMetricHistory";
import { cn } from "@/shared/lib/utils";
import { INSPECTOR_LATEST_KEY, INSPECTOR_PREVIEW_KEY } from "./keys";

interface HostRef {
  id: string;
  name: string;
}

/**
 * Optional host-kind hint used to scope the per-category fold/unfold state in
 * localStorage so a device inspector remembers a different set of collapsed
 * groups than a VM inspector. Unknown for the standalone preview panel (which
 * stores under a shared `"unknown"` bucket — acceptable since no two callers
 * compete for the same group set there).
 */
type HostKind = "device" | "vm";

interface HostMetricsPreviewSectionProps {
  host: HostRef;
  /**
   * Drives the localStorage key for the category fold/unfold preference. The
   * hidden `visible={false}` instance and the visible instance MUST receive
   * the same `kind`, or the operator's collapsed groups will not be honoured.
   */
  kind?: HostKind;
  visible?: boolean;
  enabled?: boolean;
  onConfiguredMetricsChange?: (configured: boolean) => void;
}

function categoryStorageKey(kind: HostKind | undefined): string {
  return `obs:inspector:groups:${kind ?? "unknown"}`;
}

function readCollapsedFromStorage(kind: HostKind | undefined): Record<string, boolean> {
  if (typeof window === "undefined") return {};
  try {
    const raw = window.localStorage.getItem(categoryStorageKey(kind));
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) {
      const result: Record<string, boolean> = {};
      for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
        if (typeof v === "boolean") result[k] = v;
      }
      return result;
    }
  } catch {
    // Corrupt JSON or storage disabled — fall through to defaults.
  }
  return {};
}

function writeCollapsedToStorage(
  kind: HostKind | undefined,
  collapsed: Record<string, boolean>,
): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(categoryStorageKey(kind), JSON.stringify(collapsed));
  } catch {
    // Quota / private-mode — silently drop the preference.
  }
}

/**
 * Per-kind localStorage-backed fold/unfold state. Default for a never-touched
 * category is expanded; the stored object records explicit user toggles only,
 * so adding a new category later does not surprise the operator by appearing
 * collapsed. Re-syncs on `kind` change via the render-time-update pattern (no
 * `setState`-in-`useEffect`) so the inspector — which is *not* keyed by host
 * id in `ConnectionsWorkspace` — does not show the device bucket's collapsed
 * groups when the selection moves to a VM (or vice versa).
 */
function useCategoryCollapseState(kind: HostKind | undefined): {
  isCollapsed: (key: string) => boolean;
  toggle: (key: string) => void;
} {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>(() =>
    readCollapsedFromStorage(kind),
  );
  const [trackedKind, setTrackedKind] = useState<HostKind | undefined>(kind);
  if (trackedKind !== kind) {
    setTrackedKind(kind);
    setCollapsed(readCollapsedFromStorage(kind));
  }

  // Side-effect (localStorage write) is lifted out of the updater so the
  // updater stays pure — React's StrictMode double-fires updaters in dev,
  // and writing inside the updater would persist twice per toggle.
  const toggle = useCallback(
    (key: string) => {
      setCollapsed((prev) => ({ ...prev, [key]: !prev[key] }));
    },
    [],
  );

  useEffect(() => {
    writeCollapsedToStorage(kind, collapsed);
  }, [kind, collapsed]);

  const isCollapsed = useCallback((key: string) => Boolean(collapsed[key]), [collapsed]);

  return { isCollapsed, toggle };
}

const EMPTY_METRICS: MonitoringMetricDescriptor[] = [];
const METRIC_REFRESH_MS = 5_000;
const MAX_SPARKLINE_SAMPLES = 60;
// Recent window backfilled from the history API for sparkline cards so the
// trend line is populated immediately on open, before the 5s live poll has
// accumulated enough samples on its own.
const SPARKLINE_HISTORY_RANGE_SECONDS = 60 * 60;
const SPARKLINE_HISTORY_STEP_SECONDS = 60;

// Summary bar tone is intentionally quiet to harmonize with the surrounding
// inspector sections (Информация / Детали / Контекст конфигурации): no tinted
// background or coloured border — status is carried only by the icon glyph
// (CheckCircle2 / AlertTriangle / CircleHelp) and a tiny matching colour on it.
const SUMMARY_TONE: Record<
  HostObservabilityState,
  { iconClassName: string }
> = {
  normal: { iconClassName: "text-status-positive" },
  warning: { iconClassName: "text-status-warning" },
  critical: { iconClassName: "text-status-negative" },
  unknown: { iconClassName: "text-muted-foreground" },
};

interface MetricSampleSnapshot {
  scopeKey: string;
  series: Record<string, MetricSparklinePoint[]>;
}

interface MetricSampleStore {
  getSnapshot: () => MetricSampleSnapshot;
  ingest: (input: {
    scopeKey: string;
    items: LatestMetricItem[];
    sampledAt: number;
    metrics: MonitoringMetricDescriptor[];
  }) => void;
  reset: (scopeKey: string) => void;
  subscribe: (listener: () => void) => () => void;
}

function updateMetricSampleSeries({
  previous,
  items,
  sampledAt,
  metrics,
}: {
  previous: Record<string, MetricSparklinePoint[]>;
  items: LatestMetricItem[];
  sampledAt: number;
  metrics: MonitoringMetricDescriptor[];
}): Record<string, MetricSparklinePoint[]> {
  if (items.length === 0) return previous;

  const itemsByQuery = new Map<string, LatestMetricItem>();
  for (const item of items) {
    itemsByQuery.set(item.query, item);
  }

  const next = { ...previous };
  const activeKeys = new Set<string>();
  let changed = false;

  for (const metric of metrics) {
    const key = metricKey(metric);
    activeKeys.add(key);
    const item = itemsByQuery.get(metric.query);
    if (!metricHasValue(item)) continue;
    const numeric = item ? parseMetricNumericValue(rawMetricValue(item)) : null;
    if (numeric === null) continue;

    const existing = next[key] ?? [];
    if (existing.at(-1)?.timestamp === sampledAt) continue;
    next[key] = [...existing, { timestamp: sampledAt, value: numeric }].slice(
      -MAX_SPARKLINE_SAMPLES,
    );
    changed = true;
  }

  for (const key of Object.keys(next)) {
    if (!activeKeys.has(key)) {
      delete next[key];
      changed = true;
    }
  }

  return changed ? next : previous;
}

function createMetricSampleStore(): MetricSampleStore {
  let snapshot: MetricSampleSnapshot = { scopeKey: "", series: {} };
  const listeners = new Set<() => void>();
  const emit = () => {
    for (const listener of listeners) listener();
  };

  return {
    getSnapshot: () => snapshot,
    ingest: ({ scopeKey, items, sampledAt, metrics }) => {
      const previousSeries = snapshot.scopeKey === scopeKey ? snapshot.series : {};
      const series = updateMetricSampleSeries({
        previous: previousSeries,
        items,
        sampledAt,
        metrics,
      });
      if (snapshot.scopeKey === scopeKey && series === snapshot.series) return;
      snapshot = { scopeKey, series };
      emit();
    },
    reset: (scopeKey) => {
      if (snapshot.scopeKey === scopeKey && Object.keys(snapshot.series).length === 0) return;
      snapshot = { scopeKey, series: {} };
      emit();
    },
    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

function summaryDetailKey(summary: HostObservabilitySummary): string {
  if (summary.critical > 0) return "observability.summary.criticalCount";
  if (summary.warning > 0) return "observability.summary.warningCount";
  if (summary.unknown > 0) return "observability.summary.unknownCount";
  return "observability.summary.okCount";
}

function SummaryIcon({
  state,
  className,
}: {
  state: HostObservabilityState;
  className?: string;
}) {
  if (state === "normal") return <CheckCircle2 className={className} />;
  if (state === "unknown") return <CircleHelp className={className} />;
  return <AlertTriangle className={className} />;
}

function HostObservabilitySummaryBar({
  summary,
}: {
  summary: HostObservabilitySummary;
}) {
  const { t } = useTranslation();
  const isStaleNormal = summary.stale && summary.state === "normal";
  const toneState = isStaleNormal ? "warning" : summary.state;
  const tone = SUMMARY_TONE[toneState];
  const titleKey = isStaleNormal
    ? "observability.summary.states.stale"
    : `observability.summary.states.${summary.state}`;
  let count = summary.ok;
  if (summary.critical > 0) {
    count = summary.critical;
  } else if (summary.warning > 0) {
    count = summary.warning;
  } else if (summary.unknown > 0) {
    count = summary.unknown;
  }

  return (
    <div className="flex min-h-7 items-center justify-between gap-2 px-2.5 py-1 text-xs">
      <div className="flex min-w-0 items-center gap-2">
        <SummaryIcon
          state={toneState}
          className={cn("h-3 w-3 shrink-0", tone.iconClassName)}
        />
        <span className="shrink-0 font-medium text-foreground">{t(titleKey)}</span>
        <span className="truncate text-muted-foreground">
          {t(summaryDetailKey(summary), { count, total: summary.total })}
        </span>
      </div>
      {summary.stale && (
        <span className="shrink-0 text-xs uppercase tracking-wide text-muted-foreground">
          {t("observability.summary.staleBadge")}
        </span>
      )}
    </div>
  );
}

function metricIsSparkline(metric: MonitoringMetricDescriptor): boolean {
  return (metric.widget_type ?? "").trim().toLowerCase() === "sparkline";
}

/**
 * Renders one metric card. For sparkline metrics it backfills the trend line
 * from the history API; the live 5s sample series remains the fallback while
 * history loads, errors, or returns no points.
 */
function MetricCardWithHistory({
  metric,
  latest,
  loading,
  stale,
  sampleSeries,
  sparklineLabel,
  historyEnabled,
  hostId,
  onEditOverride,
}: {
  metric: MonitoringMetricDescriptor;
  latest?: LatestMetricItem;
  loading: boolean;
  stale: boolean;
  sampleSeries?: MetricSparklinePoint[];
  sparklineLabel: string;
  historyEnabled: boolean;
  hostId: string;
  onEditOverride?: () => void;
}) {
  const { t } = useTranslation();
  const isSparkline = metricIsSparkline(metric);
  const history = useMetricHistory(hostId, metric.query, {
    rangeSeconds: SPARKLINE_HISTORY_RANGE_SECONDS,
    step: SPARKLINE_HISTORY_STEP_SECONDS,
    enabled: historyEnabled && isSparkline,
  });
  const series = isSparkline && history.points.length > 0 ? history.points : sampleSeries;

  return (
    <MetricWidgetCard
      metric={metric}
      latest={latest}
      loading={loading}
      stale={stale}
      series={series}
      sparklineLabel={sparklineLabel}
      variant="row"
      rowAction={
        onEditOverride ? (
          <Button
            type="button"
            variant="ghost"
            size="icon"
            className="h-5 w-5"
            onClick={onEditOverride}
            aria-label={t("observability.hostOverride.editAction", {
              defaultValue: "Override metric for this host",
            })}
          >
            <Pencil className="h-3 w-3" />
          </Button>
        ) : null
      }
    />
  );
}

const HostMetricsPreviewSection = ({
  host,
  kind,
  visible = true,
  enabled = true,
  onConfiguredMetricsChange,
}: HostMetricsPreviewSectionProps) => {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const { isCollapsed, toggle } = useCategoryCollapseState(kind);
  const sampleStore = useMemo(() => createMetricSampleStore(), []);
  const sampleSnapshot = useSyncExternalStore(
    sampleStore.subscribe,
    sampleStore.getSnapshot,
    sampleStore.getSnapshot,
  );

  const observabilityQuery = useQuery({
    queryKey: [...INSPECTOR_PREVIEW_KEY, host.id],
    queryFn: () => getConnectionTargetObservability(host.id),
    enabled,
  });

  const allMetrics = observabilityQuery.data?.target.metrics ?? EMPTY_METRICS;
  const inspectorMetrics = observabilityQuery.data?.target.inspector_metrics ?? EMPTY_METRICS;
  const hasMetricDescriptors = allMetrics.length > 0;
  const metricSources = useMemo(
    () => observabilityQuery.data?.target.metric_sources ?? [],
    [observabilityQuery.data],
  );
  const resolvedInspectorMetrics = useMemo(
    () => filterMetricsWithResolvedSources(inspectorMetrics, metricSources),
    [inspectorMetrics, metricSources],
  );
  const hasInspectorMetrics = resolvedInspectorMetrics.length > 0;
  const latestQueries = useMemo(
    () => resolvedInspectorMetrics.map((metric) => metric.query).filter(Boolean),
    [resolvedInspectorMetrics],
  );
  const latestQueryKey = latestQueries.join("\n");
  const canResolveLatestSource = hasMetricDescriptors && hasInspectorMetrics && latestQueries.length > 0;
  const sampleScopeKey = `${host.id}\n${latestQueryKey}`;

  const latestQuery = useQuery({
    queryKey: [...INSPECTOR_LATEST_KEY, host.id, latestQueryKey],
    queryFn: () => getLatestMetrics(latestQueries, host.id),
    enabled: enabled && visible && canResolveLatestSource,
    refetchInterval: enabled && visible && canResolveLatestSource ? METRIC_REFRESH_MS : false,
    refetchIntervalInBackground: false,
    staleTime: 0,
  });
  const hasStaleLatestError =
    (latestQuery.isRefetchError || latestQuery.isError) && Boolean(latestQuery.data);

  // The descriptor returned by the observability endpoint now carries
  // `metric_query_id` directly (single source of truth for the override
  // join key — see `MonitoringMetricDescriptor` in monitoring.rs). No
  // catalog round-trip needed; an absent id means the descriptor is not
  // backed by a persisted metric_query row and cannot be overridden.
  const [editingMetric, setEditingMetric] = useState<MonitoringMetricDescriptor | null>(null);

  useEffect(() => {
    if (!enabled || !onConfiguredMetricsChange) return;
    if (observabilityQuery.isError) {
      onConfiguredMetricsChange(false);
      return;
    }
    if (!observabilityQuery.isSuccess) return;
    if (!canResolveLatestSource) {
      onConfiguredMetricsChange(false);
      return;
    }
    if (!visible) {
      onConfiguredMetricsChange(true);
      return;
    }
    if (latestQuery.data?.configured === true) {
      onConfiguredMetricsChange((latestQuery.data.items ?? []).length > 0);
      return;
    }
    if (latestQuery.data?.configured === false || latestQuery.isError) {
      onConfiguredMetricsChange(false);
    }
  }, [
    canResolveLatestSource,
    enabled,
    latestQuery.data?.configured,
    latestQuery.data?.items,
    latestQuery.isError,
    observabilityQuery.isError,
    observabilityQuery.isSuccess,
    onConfiguredMetricsChange,
    visible,
  ]);

  const latestByQuery = useMemo(() => {
    const map = new Map<string, LatestMetricItem>();
    for (const item of latestQuery.data?.items ?? []) {
      map.set(item.query, item);
    }
    return map;
  }, [latestQuery.data?.items]);

  useEffect(() => {
    sampleStore.reset(sampleScopeKey);
  }, [sampleScopeKey, sampleStore]);

  useEffect(() => {
    sampleStore.ingest({
      scopeKey: sampleScopeKey,
      items: latestQuery.data?.items ?? [],
      sampledAt: latestQuery.dataUpdatedAt || Date.now(),
      metrics: resolvedInspectorMetrics,
    });
  }, [
    latestQuery.data?.items,
    latestQuery.dataUpdatedAt,
    resolvedInspectorMetrics,
    sampleScopeKey,
    sampleStore,
  ]);

  const sampleSeriesByMetric =
    sampleSnapshot.scopeKey === sampleScopeKey ? sampleSnapshot.series : {};

  if (!visible) return null;
  if (!enabled) return null;

  if (observabilityQuery.isPending) {
    return (
      <div className="flex items-center gap-2 py-1 text-xs text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        <span>{t("loading")}</span>
      </div>
    );
  }

  if (observabilityQuery.isError) {
    return (
      <div className="flex items-start gap-2 py-1 text-xs text-muted-foreground">
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-status-warning" />
        <span>{observabilityQuery.error.message}</span>
      </div>
    );
  }

  if (!hasMetricDescriptors || !hasInspectorMetrics || !canResolveLatestSource) {
    return null;
  }

  if (latestQuery.data?.configured === false) {
    return null;
  }

  if (latestQuery.isPending && !latestQuery.data) {
    return (
      <div className="flex items-center gap-2 py-1 text-xs text-muted-foreground">
        <Loader2 className="h-3 w-3 animate-spin" />
        <span>{t("loading")}</span>
      </div>
    );
  }

  if (latestQuery.isError && !latestQuery.data) {
    return (
      <div className="flex items-start gap-2 py-1 text-xs text-muted-foreground">
        <AlertTriangle className="mt-0.5 h-3 w-3 shrink-0 text-status-warning" />
        <span>{latestQuery.error.message}</span>
      </div>
    );
  }

  const visibleMetrics = resolvedInspectorMetrics.filter((metric) => latestByQuery.has(metric.query));
  const summary = summarizeHostObservability(
    resolvedInspectorMetrics,
    latestByQuery,
    hasStaleLatestError,
  );

  const uncategorizedTitle = t("observability.inspector.uncategorized", {
    defaultValue: "Other",
  });
  const groups = groupMetricsByCategory(visibleMetrics, uncategorizedTitle);
  // Per the spec: only render the grouping shell when more than one bucket
  // would appear. All-uncategorized (one bucket) and the empty case fall
  // through to the calm flat list — no header, no noise.
  const renderGrouped = groups.length > 1;

  const renderMetricRow = (metric: MonitoringMetricDescriptor) => {
    const key = metricKey(metric);
    const overridable = Boolean(metric.metric_query_id) && (can("CREATE", "MONITORING") || can("UPDATE", "MONITORING") || can("DELETE", "MONITORING"));
    return (
      <MetricCardWithHistory
        key={key}
        metric={metric}
        latest={latestByQuery.get(metric.query)}
        loading={latestQuery.isPending}
        stale={hasStaleLatestError}
        sampleSeries={sampleSeriesByMetric[key]}
        sparklineLabel={t("observability.metric.recentValues")}
        historyEnabled={enabled && visible}
        hostId={host.id}
        onEditOverride={overridable ? () => setEditingMetric(metric) : undefined}
      />
    );
  };

  return (
    <div className="space-y-0.5">
      <HostObservabilitySummaryBar summary={summary} />
      {renderGrouped ? (
        <div className="space-y-1">
          {groups.map((group) => {
            const collapsed = isCollapsed(group.key);
            const headerId = `obs-group-${host.id}-${group.key}`;
            const panelId = `${headerId}-panel`;
            return (
              <div key={group.key} className="space-y-0.5">
                <button
                  type="button"
                  id={headerId}
                  className="flex w-full items-center gap-1.5 rounded-sm px-2 py-1 text-xs font-medium uppercase tracking-wide text-muted-foreground transition-colors hover:bg-muted focus-visible:bg-muted focus-visible:outline-none"
                  aria-expanded={!collapsed}
                  aria-controls={panelId}
                  onClick={() => toggle(group.key)}
                >
                  {collapsed ? (
                    <ChevronRight className="h-3 w-3 shrink-0" aria-hidden />
                  ) : (
                    <ChevronDown className="h-3 w-3 shrink-0" aria-hidden />
                  )}
                  <span className="truncate">{group.title}</span>
                  <span className="ml-auto rounded-full bg-muted px-1.5 text-xs font-normal text-muted-foreground">
                    {group.metrics.length}
                  </span>
                </button>
                {!collapsed && (
                  <div
                    id={panelId}
                    role="region"
                    aria-labelledby={headerId}
                    className="space-y-0.5"
                  >
                    {group.metrics.map(renderMetricRow)}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ) : (
        <div className="space-y-0.5">{visibleMetrics.map(renderMetricRow)}</div>
      )}
      {editingMetric && editingMetric.metric_query_id ? (
        <HostMetricOverrideDrawer
          open={true}
          onOpenChange={(next) => {
            if (!next) setEditingMetric(null);
          }}
          host={host}
          metric={editingMetric}
          metricQueryId={editingMetric.metric_query_id}
        />
      ) : null}
    </div>
  );
};

export default HostMetricsPreviewSection;
