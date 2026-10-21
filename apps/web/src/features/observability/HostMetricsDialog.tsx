import { useEffect, useMemo, useRef, useState } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertTriangle,
  ChevronDown,
  Gauge,
  LineChart,
  Loader2,
  Radio,
} from "lucide-react";

import EmptyState from "@/shared/ui-shared/EmptyState";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import {
  getConnectionTargetObservability,
  getLatestMetrics,
  type MonitoringMetricDescriptor,
} from "@/features/observability/api";
import {
  filterMetricsWithResolvedSources,
  metricValueSummary,
} from "@/features/observability/metricHelpers";
import MetricHistoryChart from "@/features/observability/MetricHistoryChart";
import { cn } from "@/shared/lib/utils";

interface HostRef {
  id: string;
  name?: string;
}

interface HostMetricsDialogProps {
  host: HostRef | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

const EMPTY_METRICS: MonitoringMetricDescriptor[] = [];

const HostMetricsDialog = ({ host, open, onOpenChange }: HostMetricsDialogProps) => {
  const { t } = useTranslation();
  const [expandedQuery, setExpandedQuery] = useState<string | null>(null);
  // Reset the expanded chart during render when the dialog target changes,
  // instead of in an effect (avoids react-hooks/set-state-in-effect).
  const expandedScopeRef = useRef<string | null>(null);
  const expandedScope = open ? (host?.id ?? null) : null;
  if (expandedScopeRef.current !== expandedScope) {
    expandedScopeRef.current = expandedScope;
    if (expandedQuery !== null) setExpandedQuery(null);
  }

  const observabilityQuery = useQuery({
    queryKey: ["observability", "connection-target", host?.id],
    queryFn: () => getConnectionTargetObservability(host?.id ?? ""),
    enabled: open && !!host?.id,
  });

  const metrics = observabilityQuery.data?.target.metrics ?? EMPTY_METRICS;
  const metricSources = useMemo(
    () => observabilityQuery.data?.target.metric_sources ?? [],
    [observabilityQuery.data],
  );
  const metricsWithResolvedSources = useMemo(
    () => filterMetricsWithResolvedSources(metrics, metricSources),
    [metrics, metricSources],
  );
  const latestQueries = useMemo(
    () => metricsWithResolvedSources.map((metric) => metric.query).filter(Boolean),
    [metricsWithResolvedSources],
  );
  const latestSourceAvailable = latestQueries.length > 0;

  const latestMutation = useMutation({
    mutationFn: () => getLatestMetrics(latestQueries, host?.id),
  });
  const resetLatestMetricsRef = useRef(latestMutation.reset);

  useEffect(() => {
    resetLatestMetricsRef.current = latestMutation.reset;
  }, [latestMutation.reset]);

  useEffect(() => {
    resetLatestMetricsRef.current();
  }, [host?.id, open]);

  const handleOpenChange = (nextOpen: boolean) => {
    onOpenChange(nextOpen);
  };

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent
        className="border border-border bg-card max-h-[86vh] max-w-5xl overflow-hidden p-0"
        description={t("observability.hostDialogDescription")}
      >
        <DialogHeader className="border-b border-border px-5 py-4">
          <DialogTitle className="flex items-center gap-2 text-base">
            <Gauge className="h-4 w-4 text-primary" />
            {host?.name ?? t("observability.host")}
          </DialogTitle>
        </DialogHeader>

        {observabilityQuery.isPending ? (
          <div className="flex h-80 items-center justify-center text-xs text-muted-foreground">
            <Loader2 className="mr-2 h-4 w-4 animate-spin" />
            {t("loading")}
          </div>
        ) : observabilityQuery.isError ? (
          <div className="p-6">
            <EmptyState
              icon={AlertTriangle}
              title={t("dataTable.errorTitle")}
              description={observabilityQuery.error.message}
              actionLabel={t("retry")}
              onAction={() => observabilityQuery.refetch()}
            />
          </div>
        ) : (
          <div className="grid min-h-0 gap-0 overflow-hidden lg:grid-cols-[minmax(0,1fr)_20rem]">
            <div className="max-h-[72vh] overflow-auto p-5">
              <div className="grid gap-4">
                <section className="grid gap-2">
                  <div className="flex items-center gap-2">
                    <Radio className="h-4 w-4 text-muted-foreground" />
                    <h3 className="text-sm font-semibold">{t("observability.scrapes")}</h3>
                    <Badge variant="secondary" className="rounded-md text-xs">
                      {observabilityQuery.data?.targets_count ?? 0}
                    </Badge>
                  </div>
                  <div className="grid gap-2">
                    {(observabilityQuery.data?.target.scrapes ?? []).length > 0 ? (
                      observabilityQuery.data?.target.scrapes.map((scrape, idx) => (
                        <div key={`${scrape.name ?? "scrape"}-${idx}`} className="rounded-lg border border-border p-3">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="text-sm font-medium">{scrape.name ?? t("observability.scrape")}</span>
                            {scrape.job && (
                              <Badge variant="secondary" className="rounded-md text-xs">
                                {scrape.job}
                              </Badge>
                            )}
                            {scrape.port && (
                              <code className="text-xs text-muted-foreground">:{scrape.port}</code>
                            )}
                          </div>
                          <p className="mt-2 break-all text-xs text-muted-foreground">
                            {scrape.target ?? observabilityQuery.data?.target.host_address}
                            {scrape.metrics_path ? ` ${scrape.metrics_path}` : ""}
                          </p>
                        </div>
                      ))
                    ) : (
                      <div className="rounded-lg border border-border p-3">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="text-sm font-medium">{t("observability.defaultScrape")}</span>
                        </div>
                        <p className="mt-2 break-all text-xs text-muted-foreground">
                          {observabilityQuery.data?.groups.flatMap((group) => group.targets).join(", ") || "—"}
                        </p>
                      </div>
                    )}
                  </div>
                </section>

                <section className="grid gap-2">
                  <div className="flex items-center gap-2">
                    <Activity className="h-4 w-4 text-muted-foreground" />
                    <h3 className="text-sm font-semibold">{t("observability.metrics")}</h3>
                    <Badge variant="secondary" className="rounded-md text-xs">
                      {metrics.length}
                    </Badge>
                  </div>
                  {metrics.length === 0 ? (
                    <EmptyState
                      icon={Activity}
                      title={t("observability.noMetricDescriptors")}
                      description={t("observability.noMetricDescriptorsDescription")}
                    />
                  ) : (
                    <div className="grid gap-2">
                      {metrics.map((metric) => {
                        const rowKey = `${metric.name}:${metric.query}`;
                        const canInspect = Boolean(host?.id && metric.query);
                        const expanded = expandedQuery === metric.query;
                        return (
                          <div
                            key={rowKey}
                            className="rounded-lg border border-border"
                          >
                            <button
                              type="button"
                              disabled={!canInspect}
                              onClick={() =>
                                setExpandedQuery((current) =>
                                  current === metric.query ? null : metric.query,
                                )
                              }
                              aria-expanded={expanded}
                              className={cn(
                                "flex w-full items-start gap-3 p-3 text-left",
                                canInspect && "transition-colors hover:bg-accent",
                                !canInspect && "cursor-default",
                              )}
                            >
                              {canInspect && (
                                <ChevronDown
                                  className={cn(
                                    "mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform",
                                    expanded && "rotate-180",
                                  )}
                                />
                              )}
                              <span className="min-w-0 flex-1">
                                <span className="block text-sm font-medium">
                                  {metric.label ?? metric.name}
                                </span>
                                <code className="mt-1 block truncate text-xs text-muted-foreground">
                                  {metric.query}
                                </code>
                              </span>
                              {metric.unit && (
                                <Badge variant="outline" className="rounded-md text-xs">
                                  {metric.unit}
                                </Badge>
                              )}
                              {metric.value_type && (
                                <Badge variant="outline" className="rounded-md text-xs">
                                  {metric.value_type}
                                </Badge>
                              )}
                              {metric.widget_type && (
                                <Badge variant="outline" className="rounded-md text-xs">
                                  {metric.widget_type}
                                </Badge>
                              )}
                              {metric.source_type && (
                                <Badge variant="secondary" className="rounded-md text-xs">
                                  {metric.source_type}
                                </Badge>
                              )}
                            </button>
                            {expanded && canInspect && host?.id && (
                              <div className="border-t border-border p-3">
                                <div className="mb-2 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
                                  <LineChart className="h-3.5 w-3.5" />
                                  {t("observability.history.title")}
                                </div>
                                <MetricHistoryChart targetId={host.id} metric={metric} />
                              </div>
                            )}
                          </div>
                        );
                      })}
                    </div>
                  )}
                </section>
              </div>
            </div>

            <aside className="flex min-h-0 flex-col border-t border-border bg-accent p-4 lg:border-l lg:border-t-0">
              {latestSourceAvailable && (
                <Button
                  className="gap-1.5"
                  disabled={latestQueries.length === 0 || latestMutation.isPending}
                  onClick={() => latestMutation.mutate()}
                >
                  {latestMutation.isPending ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <Activity className="h-3.5 w-3.5" />
                  )}
                  {t("observability.showLatest")}
                </Button>
              )}

              <div className="mt-4 min-h-0 flex-1 overflow-auto">
                {latestMutation.data?.configured === false ? (
                  <EmptyState
                    icon={Gauge}
                    title={t("observability.queryDisabled")}
                    description={latestMutation.data.message ?? t("observability.queryDisabledDescription")}
                  />
                ) : latestMutation.data ? (
                  <div className="grid gap-2">
                    {latestMutation.data.items.map((item) => (
                      <div key={item.query} className="rounded-lg border border-border bg-background p-3">
                        <div className="flex items-center gap-2">
                          <code className="min-w-0 flex-1 truncate text-xs text-muted-foreground">
                            {item.query}
                          </code>
                          {item.source_type && (
                            <Badge variant="secondary" className="rounded-md text-xs">
                              {item.source_type}
                            </Badge>
                          )}
                        </div>
                        <p className="mt-2 break-all text-sm font-semibold">{metricValueSummary(item)}</p>
                      </div>
                    ))}
                  </div>
                ) : (
                  <EmptyState
                    icon={Activity}
                    title={t("observability.noLatest")}
                    description={t("observability.noLatestDescription")}
                  />
                )}
              </div>
            </aside>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
};

export default HostMetricsDialog;
