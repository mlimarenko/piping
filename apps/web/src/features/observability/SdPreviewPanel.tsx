import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { AlertTriangle, Copy, Eye, Gauge, Loader2, RefreshCw } from "lucide-react";

import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import { notify } from "@/shared/lib/notifications";
import {
  getServiceDiscoveryPreview,
  serviceDiscoveryCollectorUrl,
  type ServiceDiscoveryTargetGroup,
} from "@/features/observability/api";

function groupTitle(group: ServiceDiscoveryTargetGroup, fallback: string): string {
  const labels = group.labels;
  return (
    labels.piping_site ??
    labels.piping_tenant ??
    labels.piping_role ??
    labels.piping_host_name ??
    labels.job ??
    fallback
  );
}

/**
 * Live preview of the Service Discovery targets Prometheus will scrape, plus
 * the collector endpoint and a ready-to-paste `http_sd_configs` snippet.
 * Read-only — it reflects whatever the exporter assignments currently produce.
 */
const SdPreviewPanel = () => {
  const { t } = useTranslation();
  const [previewEnabled, setPreviewEnabled] = useState(false);
  const previewQuery = useQuery({
    queryKey: ["observability", "service-discovery-preview", "sample"],
    queryFn: () => getServiceDiscoveryPreview({ limit: 200 }),
    enabled: previewEnabled,
    retry: false,
  });

  const collectorUrl = serviceDiscoveryCollectorUrl();
  const scrapeSnippet = `- job_name: piping-discovery
  http_sd_configs:
    - url: "${collectorUrl}"
      refresh_interval: 30s`;

  const [copyState, setCopyState] = useState<"url" | "snippet" | "output" | null>(null);
  const copy = async (text: string, which: "url" | "snippet" | "output") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopyState(which);
      window.setTimeout(() => setCopyState(null), 1500);
      notify("success", t("copied", { defaultValue: "Copied" }));
    } catch {
      notify("error", t("observability.workflow.copyFailed", { defaultValue: "Could not copy. Select and copy the value manually." }));
    }
  };

  const data = previewQuery.data;
  const hosts = data?.hosts ?? [];
  const groups = data?.groups ?? [];
  const scrapeTotal = hosts.reduce((sum, host) => sum + Math.max(1, host.scrapes.length), 0);
  const targetGroupFallback = t("observability.sd.targetGroup", { defaultValue: "Target group" });

  return (
    <SurfaceCard className="min-w-0 overflow-hidden">
      <div className="border-b border-border px-3 py-3">
        <div className="flex items-start justify-between gap-3">
          <div>
            <h2 className="text-sm font-semibold text-foreground">
              {t("observability.sd.previewTitle", { defaultValue: "Discovery preview" })}
            </h2>
            <p className="text-xs text-muted-foreground">
              {t("observability.sd.previewHint", {
                defaultValue: "Endpoint and a bounded sample of emitted targets.",
              })}
            </p>
          </div>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("retry", { defaultValue: "Retry" })}
            disabled={!previewEnabled || previewQuery.isFetching}
            onClick={() => previewQuery.refetch()}
          >
            <RefreshCw className={previewQuery.isFetching ? "h-3.5 w-3.5 animate-spin" : "h-3.5 w-3.5"} />
          </Button>
        </div>
      </div>

      <div className="space-y-3 p-3">
        <div className="flex items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground">
            {t("observability.collectorEndpoint", { defaultValue: "Collector endpoint" })}
          </span>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("copy", { defaultValue: "Copy" })}
            onClick={() => void copy(collectorUrl, "url")}
          >
            <Copy className={copyState === "url" ? "h-3.5 w-3.5 text-status-positive" : "h-3.5 w-3.5"} />
          </Button>
        </div>
        <code className="block break-all rounded-md border border-border bg-accent p-2 text-xs text-muted-foreground">
          {collectorUrl}
        </code>

        <div className="flex items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground">
            {t("observability.scrapeConfig", { defaultValue: "Scrape config" })}
          </span>
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label={t("copy", { defaultValue: "Copy" })}
            onClick={() => void copy(scrapeSnippet, "snippet")}
          >
            <Copy
              className={copyState === "snippet" ? "h-3.5 w-3.5 text-status-positive" : "h-3.5 w-3.5"}
            />
          </Button>
        </div>
        <pre className="overflow-auto rounded-md border border-border bg-accent p-2 text-xs text-muted-foreground">
          {scrapeSnippet}
        </pre>

        {!previewEnabled ? (
          <div className="rounded-md border border-dashed border-border bg-background p-3 text-center">
            <Gauge className="mx-auto h-5 w-5 text-primary" />
            <p className="mt-2 text-xs font-semibold text-foreground">
              {t("observability.sd.previewDeferredTitle", {
                defaultValue: "Preview is loaded on demand",
              })}
            </p>
            <p className="mx-auto mt-1 max-w-[24rem] text-xs leading-5 text-muted-foreground">
              {t("observability.sd.previewDeferredHint", {
                defaultValue:
                  "Large inventories can emit thousands of targets. Load a bounded sample when you need to inspect the output.",
              })}
            </p>
            <Button
              className="mt-3 gap-1.5"
              onClick={() => setPreviewEnabled(true)}
            >
              <Eye className="h-3.5 w-3.5" />
              {t("observability.sd.loadPreview", { defaultValue: "Load sample" })}
            </Button>
          </div>
        ) : (
          <>
            <div className="grid grid-cols-2 gap-2">
              {[
                { label: t("observability.targets", { defaultValue: "Targets" }), value: data?.targets_count },
                {
                  label: t("observability.targetGroups", { defaultValue: "Groups" }),
                  value: data?.groups_count,
                },
                {
                  label: t("observability.hosts", { defaultValue: "Inventory targets" }),
                  value: hosts.length,
                },
                { label: t("observability.scrapes", { defaultValue: "Scrapes" }), value: scrapeTotal },
              ].map((stat) => (
                <div
                  key={stat.label}
                  className="rounded-md border border-border bg-background p-2"
                >
                  <p className="break-words text-xs uppercase text-muted-foreground">{stat.label}</p>
                  <p className="mt-1 text-lg font-semibold tabular-nums">{data && !previewQuery.isError ? stat.value ?? "—" : "—"}</p>
                </div>
              ))}
            </div>

            <div className="max-h-[22rem] overflow-auto rounded-md border border-border">
              {previewQuery.isPending ? (
                <div className="flex items-center justify-center py-10 text-xs text-muted-foreground">
                  <Loader2 className="mr-2 h-4 w-4 animate-spin" />
                  {t("loading", { defaultValue: "Loading…" })}
                </div>
              ) : previewQuery.isError ? (
                <EmptyState
                  icon={AlertTriangle}
                  title={t("dataTable.errorTitle", { defaultValue: "Failed to load" })}
                  description={t("observability.entityLoadFailed")}
                  actionLabel={t("retry", { defaultValue: "Retry" })}
                  onAction={() => previewQuery.refetch()}
                />
              ) : groups.length === 0 ? (
                <EmptyState
                  icon={Gauge}
                  title={t("observability.noTargets", { defaultValue: "No targets yet" })}
                  description={t("observability.noTargetsDescription", {
                    defaultValue: "Create an exporter assignment to start discovering targets.",
                  })}
                />
              ) : (
                <div className="divide-y divide-border">
                  {groups.map((group, idx) => (
                    <div
                      key={`${groupTitle(group, targetGroupFallback)}-${idx}`}
                      className="grid gap-2 p-3"
                    >
                      <div className="min-w-0">
                        <p className="break-all text-xs font-semibold">
                          {groupTitle(group, targetGroupFallback)}
                        </p>
                        <p className="text-xs text-muted-foreground">
                          {t("observability.targetsCount", {
                            count: group.targets.length,
                            defaultValue: `${group.targets.length} targets`,
                          })}
                        </p>
                      </div>
                      <div className="flex flex-wrap gap-1">
                        {Object.entries(group.labels)
                          .sort(([a], [b]) => a.localeCompare(b))
                          .map(([key, value]) => (
                            <Badge key={key} variant="secondary" className="max-w-full whitespace-normal break-all rounded-md text-xs">
                              {key}: {value}
                            </Badge>
                          ))}
                      </div>
                      <div className="flex flex-wrap gap-1">
                        {group.targets.map((target) => (
                          <code
                            key={target}
                            className="max-w-full whitespace-normal break-all rounded-md bg-accent px-1.5 py-1 text-xs text-muted-foreground"
                          >
                            {target}
                          </code>
                        ))}
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
            {data && !previewQuery.isError && <Button variant="outline" onClick={() => void copy(JSON.stringify(data, null, 2), "output")}><Copy className="size-4" />{t("observability.workflow.copyOutput", { defaultValue: "Copy sample output" })}</Button>}
          </>
        )}
      </div>
    </SurfaceCard>
  );
};

export default SdPreviewPanel;
