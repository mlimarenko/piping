import { Fragment, useMemo } from "react";
import type { TFunction } from "i18next";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Activity,
  AlertTriangle,
  CircleHelp,
  Laptop,
  Loader2,
  Monitor,
  Server,
  Waypoints,
  Wifi,
  WifiOff,
  type LucideIcon,
} from "lucide-react";

import {
  getSessionHealth,
  type ConnectionSessionHealth,
  type ConnectionSessionHealthLeg,
} from "@/features/connections/api";
import type { ViewerSessionStatus } from "@/shared/api/types";
import { useConnectionLossWatch } from "./connectionLossWatch";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import { cn } from "@/shared/lib/utils";
import {
  formatRouteLatency,
  getHealthRouteEdgeEndpoints,
  getHealthTopology,
  getWorstHealthLeg,
  isMeasuredLatency,
  normalizeHealthState,
  projectHealthSnapshot,
  withClientBackendLatency,
  type HealthRouteNodeId,
  type HealthTopologyEdge,
  type NormalizedHealthState,
} from "./sessionHealthProjection";

const HEALTH_POLL_INTERVAL_MS = 1_000;

interface SessionHealthBarProps {
  sessionId: string;
  enabled?: boolean;
  className?: string;
  viewerStatus?: ViewerSessionStatus;
  /**
   * Fired once when the authoritative session health reports the user↔backend
   * leg down while the viewer still believes it is `connected` (see
   * {@link useConnectionLossWatch}). This is the recovery signal for a half-open
   * transport that the Guacamole/noVNC viewer cannot detect itself — the client
   * WebSocket stays open after the backend's client bridge dies, so the frozen
   * canvas never fires its own `onClose`.
   */
  onConnectionLost?: () => void;
}

const STATE_TONE: Record<NormalizedHealthState, string> = {
  ok: "text-status-positive",
  connecting: "text-status-warning",
  degraded: "text-status-warning",
  disconnected: "text-muted-foreground",
  failed: "text-status-negative",
  unknown: "text-muted-foreground",
};

const STATE_DOT: Record<NormalizedHealthState, string> = {
  ok: "bg-status-positive",
  connecting: "bg-status-warning",
  degraded: "bg-status-warning",
  disconnected: "bg-muted-foreground",
  failed: "bg-status-negative",
  unknown: "bg-muted-foreground",
};

const STATE_ICON: Record<NormalizedHealthState, LucideIcon> = {
  ok: Wifi,
  connecting: Loader2,
  degraded: Activity,
  disconnected: WifiOff,
  failed: AlertTriangle,
  unknown: CircleHelp,
};

const NODE_ICONS: Record<HealthRouteNodeId, LucideIcon> = {
  user: Laptop,
  piping: Server,
  proxy: Waypoints,
  host: Monitor,
};

function SessionHealthBar({
  sessionId,
  enabled = true,
  className,
  viewerStatus,
  onConnectionLost,
}: SessionHealthBarProps) {
  const { t } = useTranslation();
  const query = useQuery({
    queryKey: ["connections", "sessions", sessionId, "health"],
    queryFn: async () => {
      const startedAt = nowForLatency();
      const health = await getSessionHealth(sessionId);
      const endedAt = nowForLatency();
      return withClientBackendLatency(
        health,
        getHealthRequestLatencyMs(sessionId, startedAt, endedAt) ?? endedAt - startedAt,
      );
    },
    enabled: enabled && Boolean(sessionId),
    refetchInterval: enabled ? HEALTH_POLL_INTERVAL_MS : false,
    staleTime: HEALTH_POLL_INTERVAL_MS,
    retry: 1,
  });

  const viewModel = useMemo(
    () =>
      query.data
        ? projectHealthSnapshot(query.data, query.dataUpdatedAt, Date.now(), viewerStatus)
        : null,
    [query.data, query.dataUpdatedAt, viewerStatus],
  );

  // Half-open transport recovery: a Guacamole/noVNC viewer keeps showing the
  // last frame when its client WebSocket stays open after the backend's client
  // bridge dies — it never fires `onClose`, so the viewer stays "connected".
  // Drive recovery off the authoritative health poll instead.
  useConnectionLossWatch(query.data, query.dataUpdatedAt, viewerStatus, onConnectionLost);

  if (!enabled) return null;

  if (!viewModel) {
    const state: NormalizedHealthState = query.isError ? "unknown" : "connecting";
    const Icon = STATE_ICON[state];
    return (
      <div
        className={cn(
          "flex size-9 shrink-0 items-center justify-center text-xs text-muted-foreground",
          className,
        )}
        data-testid="session-health-bar"
      >
        <Icon className={cn("h-3.5 w-3.5", query.isFetching && !query.isError && "animate-spin")} />
        <span className="sr-only">
          {query.isError ? t("connections.health.unavailable") : t("connections.health.loading")}
        </span>
      </div>
    );
  }

  return (
    <SessionHealthBarView
      health={viewModel.health}
      state={viewModel.state}
      stale={viewModel.stale}
      className={className}
    />
  );
}

function nowForLatency(): number {
  return typeof performance !== "undefined" && typeof performance.now === "function"
    ? performance.now()
    : Date.now();
}

function getHealthRequestLatencyMs(
  sessionId: string,
  startedAt: number,
  endedAt: number,
): number | null {
  if (typeof performance === "undefined" || typeof performance.getEntriesByType !== "function") {
    return null;
  }

  const expectedPath = `/api/v1/connections/sessions/${encodeURIComponent(sessionId)}/health`;
  const entries = performance.getEntriesByType("resource");
  for (let index = entries.length - 1; index >= 0; index -= 1) {
    const entry = entries[index] as PerformanceResourceTiming | undefined;
    if (!entry || !isHealthRequestEntry(entry, expectedPath, startedAt, endedAt)) continue;

    const requestToFirstByte = entry.responseStart - entry.requestStart;
    if (entry.requestStart > 0 && requestToFirstByte >= 0) return requestToFirstByte;
    if (entry.duration > 0) return entry.duration;
    return null;
  }
  return null;
}

function isHealthRequestEntry(
  entry: PerformanceResourceTiming,
  expectedPath: string,
  startedAt: number,
  endedAt: number,
): boolean {
  if (entry.startTime + 1 < startedAt || entry.startTime - 1 > endedAt) return false;
  try {
    const baseHref = typeof location === "undefined" ? "http://localhost/" : location.href;
    return new URL(entry.name, baseHref).pathname === expectedPath;
  } catch {
    return entry.name.includes(expectedPath);
  }
}

function SessionHealthBarView({
  health,
  state,
  stale,
  className,
}: {
  health: ConnectionSessionHealth;
  state: NormalizedHealthState;
  stale: boolean;
  className?: string;
}) {
  const { t } = useTranslation();
  const Icon = STATE_ICON[state];
  const routeLatency = formatRouteLatency(health.legs);
  const topology = getHealthTopology(health);
  const worstLeg = stale ? null : getWorstHealthLeg(health);
  const proxyNodeName = health.proxy_node_name ?? undefined;
  const proxyNodeLabel = proxyNodeName ? formatProxyNodeLabel(t, proxyNodeName) : undefined;
  const diagnosis = formatHealthDiagnosis(t, state, stale, worstLeg, routeLatency, proxyNodeName);
  const compactEdgeState = stale ? "unknown" : undefined;
  const topologyLabel = formatTopologyLabel(t, topology, compactEdgeState, proxyNodeName);

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          className={cn(
            "flex size-9 shrink-0 appearance-none items-center justify-center rounded-md text-xs hover:bg-accent",
            "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/50",
            className,
          )}
          aria-label={`${diagnosis}. ${topologyLabel}`}
          title={diagnosis}
          data-testid="session-health-bar"
        >
          <div
            className={cn("flex items-center", STATE_TONE[state])}
          >
            <Icon className={cn("h-3.5 w-3.5 shrink-0", state === "connecting" && "animate-spin")} />
            <span className="sr-only">{diagnosis}</span>
          </div>
        </button>
      </PopoverTrigger>
      <PopoverContent side="bottom" align="end" aria-label={t("connections.health.title")} className="max-h-[min(32rem,80dvh)] w-[min(32rem,calc(100vw-1rem))] overflow-y-auto p-3">
        <div className="space-y-3">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="text-xs font-semibold text-foreground">
                {t("connections.health.title")}
              </div>
              <div className="truncate text-xs text-muted-foreground">
                {proxyNodeLabel
                  ? t("connections.health.viaProxy", { proxy: proxyNodeLabel })
                  : t(`connections.health.route.${health.route_mode === "proxied" ? "proxied" : "direct"}`)}
              </div>
            </div>
            <div className={cn("flex shrink-0 items-center gap-1 text-xs", STATE_TONE[state])}>
              <Icon className={cn("h-3.5 w-3.5", state === "connecting" && "animate-spin")} />
              <span>{stale ? t("connections.health.stale") : t(`connections.health.states.${state}`)}</span>
            </div>
          </div>
          <div className="rounded-md border border-border/50 bg-background px-2 py-1.5">
            <div className="truncate text-xs uppercase text-muted-foreground/65">
              {t("connections.health.diagnosisTitle")}
            </div>
            <div className={cn("truncate text-xs font-medium", STATE_TONE[state])}>
              {diagnosis}
            </div>
          </div>
          <HealthTopologyMap
            edges={topology}
            proxyNodeName={proxyNodeName}
            stateOverride={compactEdgeState}
            labelMode="always"
            ariaLabel={topologyLabel}
            className="rounded-md border border-border/50 bg-background px-2 py-2"
          />
          <div className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-3 rounded-md border border-border/50 bg-background px-2 py-1.5">
            <div className="min-w-0">
              <div className="truncate text-xs uppercase text-muted-foreground/65">
                {t("connections.health.totalLatency")}
              </div>
              <div className="truncate text-xs text-muted-foreground">
                {t("connections.health.totalLatencyDescription")}
              </div>
            </div>
            <div className="font-mono text-xs tabular-nums text-foreground/85">
              {routeLatency}
            </div>
          </div>
          <div className="grid gap-2">
            {health.legs.map((leg) => (
              <HealthLegDetails key={leg.id} leg={leg} proxyNodeName={proxyNodeName} />
            ))}
          </div>
          {health.reason_code ? (
            <div className="truncate border-t border-border/50 pt-2 font-mono text-xs text-muted-foreground">
              {health.reason_code}
            </div>
          ) : null}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function HealthTopologyMap({
  edges,
  proxyNodeName,
  stateOverride,
  labelMode,
  ariaLabel,
  className,
}: {
  edges: HealthTopologyEdge[];
  proxyNodeName?: string;
  stateOverride?: NormalizedHealthState;
  labelMode: "always" | "responsive";
  ariaLabel: string;
  className?: string;
}) {
  const nodeIds =
    edges.length > 0
      ? [edges[0]!.from, ...edges.map((edge) => edge.to)]
      : (["user", "piping", "host"] satisfies HealthRouteNodeId[]);

  return (
    <div
      className={cn("flex min-w-0 shrink-0 items-center gap-1.5", className)}
      role="img"
      aria-label={ariaLabel}
      title={ariaLabel}
    >
      {nodeIds.map((nodeId, index) => (
        <Fragment key={nodeId}>
          <HealthRouteNode nodeId={nodeId} proxyNodeName={proxyNodeName} labelMode={labelMode} />
          {index < edges.length ? (
            <HealthRouteEdge
              edge={edges[index]!}
              proxyNodeName={proxyNodeName}
              stateOverride={stateOverride}
            />
          ) : null}
        </Fragment>
      ))}
    </div>
  );
}

function HealthRouteNode({
  nodeId,
  proxyNodeName,
  labelMode,
}: {
  nodeId: HealthRouteNodeId;
  proxyNodeName?: string;
  labelMode: "always" | "responsive";
}) {
  const { t } = useTranslation();
  const Icon = NODE_ICONS[nodeId];
  const label = getNodeLabel(t, nodeId, proxyNodeName);

  return (
    <span
      className="flex min-w-0 shrink-0 items-center gap-1 text-muted-foreground/80"
      title={label}
      aria-hidden="true"
    >
      <Icon className="h-3.5 w-3.5 shrink-0" />
      <span
        className={cn(
          "min-w-0 truncate text-xs text-foreground/80",
          nodeId === "proxy" ? "max-w-[7.5rem]" : "max-w-[4rem]",
          labelMode === "responsive" && "hidden",
        )}
      >
        {label}
      </span>
    </span>
  );
}

function HealthRouteEdge({
  edge,
  proxyNodeName,
  stateOverride,
}: {
  edge: HealthTopologyEdge;
  proxyNodeName?: string;
  stateOverride?: NormalizedHealthState;
}) {
  const { t } = useTranslation();
  const state = stateOverride ?? normalizeHealthState(edge.leg?.state);
  const segment = getLegLabel(t, edge.id, proxyNodeName);
  const stateLabel = t(`connections.health.states.${state}`);

  return (
    <span
      className="flex w-6 shrink-0 items-center sm:w-8"
      title={`${segment}: ${stateLabel}`}
      aria-hidden="true"
    >
      <span className={cn("h-0.5 w-full rounded-full", STATE_DOT[state])} />
    </span>
  );
}

function HealthSegmentLabel({
  leg,
  proxyNodeName,
  showMetric = true,
}: {
  leg: ConnectionSessionHealthLeg;
  proxyNodeName?: string;
  showMetric?: boolean;
}) {
  const { t } = useTranslation();
  const state = normalizeHealthState(leg.state);
  return (
    <div className="flex min-w-0 items-center gap-1.5">
      <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", STATE_DOT[state])} />
      <span className="min-w-0 truncate text-foreground/85">
        {getLegLabel(t, leg.id, proxyNodeName)}
      </span>
      {showMetric ? (
        <span className="shrink-0 text-xs text-muted-foreground">
          {formatLegMetric(leg)}
        </span>
      ) : null}
    </div>
  );
}

function HealthLegDetails({
  leg,
  proxyNodeName,
}: {
  leg: ConnectionSessionHealthLeg;
  proxyNodeName?: string;
}) {
  const { t } = useTranslation();
  const state = normalizeHealthState(leg.state);
  return (
    <div className="grid grid-cols-[minmax(0,1fr)_auto] gap-2 rounded-md border border-border/50 bg-background px-2 py-1.5">
      <div className="min-w-0">
        <div className="flex min-w-0 items-center gap-1.5">
          <HealthSegmentLabel leg={leg} proxyNodeName={proxyNodeName} />
          <span className={cn("shrink-0 text-xs", STATE_TONE[state])}>
            {t(`connections.health.states.${state}`)}
          </span>
        </div>
        <div className="mt-1 grid grid-cols-3 gap-2 text-xs text-muted-foreground">
          <Metric label={t("connections.health.latency")} value={formatLatency(leg.latency_ms)} />
          <Metric label={t("connections.health.rx")} value={formatBytes(leg.rx_bytes)} />
          <Metric label={t("connections.health.tx")} value={formatBytes(leg.tx_bytes)} />
        </div>
      </div>
      <div className="self-center text-right text-xs text-muted-foreground">
        <div>{formatBps(leg.rx_bps)}</div>
        <div>{formatBps(leg.tx_bps)}</div>
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div className="min-w-0">
      <div className="truncate uppercase text-muted-foreground/65">{label}</div>
      <div className="truncate font-mono text-foreground/80">{value}</div>
    </div>
  );
}

function getLegLabel(t: TFunction, legId: string, proxyNodeName?: string): string {
  const endpoints = getHealthRouteEdgeEndpoints(legId);
  if (endpoints) {
    return [
      getNodeLabel(t, endpoints.from, proxyNodeName),
      getNodeLabel(t, endpoints.to, proxyNodeName),
    ].join(getSegmentSeparator(t));
  }
  return String(t("connections.health.legs.unknown"));
}

function getNodeLabel(t: TFunction, nodeId: HealthRouteNodeId, proxyNodeName?: string): string {
  if (nodeId === "proxy" && proxyNodeName) return formatProxyNodeLabel(t, proxyNodeName);
  return String(t(`connections.health.nodes.${nodeId}`));
}

function formatProxyNodeLabel(t: TFunction, proxyNodeName: string): string {
  return String(t("connections.health.proxyNodeLabel", { proxy: proxyNodeName }));
}

function getSegmentSeparator(t: TFunction): string {
  return String(t("connections.health.segmentSeparator"));
}

function formatTopologyLabel(
  t: TFunction,
  edges: HealthTopologyEdge[],
  stateOverride: NormalizedHealthState | undefined,
  proxyNodeName: string | undefined,
): string {
  const nodeIds =
    edges.length > 0
      ? [edges[0]!.from, ...edges.map((edge) => edge.to)]
      : (["user", "piping", "host"] satisfies HealthRouteNodeId[]);
  const route = nodeIds
    .map((nodeId) => getNodeLabel(t, nodeId, proxyNodeName))
    .join(getSegmentSeparator(t));
  const edgeStates = edges
    .map((edge) => {
      const state = stateOverride ?? normalizeHealthState(edge.leg?.state);
      return `${getLegLabel(t, edge.id, proxyNodeName)}: ${t(`connections.health.states.${state}`)}`;
    })
    .join("; ");

  return edgeStates
    ? `${t("connections.health.topologyLabel")}: ${route}. ${edgeStates}`
    : `${t("connections.health.topologyLabel")}: ${route}`;
}

function formatHealthDiagnosis(
  t: TFunction,
  state: NormalizedHealthState,
  stale: boolean,
  worstLeg: ConnectionSessionHealthLeg | null,
  routeLatency: string,
  proxyNodeName: string | undefined,
): string {
  const latencySuffix = routeLatency === "-" ? "" : ` \u00b7 ${routeLatency}`;
  if (stale) return `${t("connections.health.diagnosis.stale")}${latencySuffix}`;
  if (state === "ok") return `${t("connections.health.diagnosis.stable")}${latencySuffix}`;

  if (state === "degraded" || state === "disconnected" || state === "failed") {
    const segment = worstLeg
      ? getLegLabel(t, worstLeg.id, proxyNodeName)
      : t("connections.health.legs.unknown");
    return `${t(`connections.health.diagnosis.${state}`, { segment })}${latencySuffix}`;
  }

  return `${t(`connections.health.states.${state}`)}${latencySuffix}`;
}

function formatLegMetric(leg: ConnectionSessionHealthLeg): string {
  if (isMeasuredLatency(leg.latency_ms)) return formatLatency(leg.latency_ms);
  if (typeof leg.rx_bps === "number" || typeof leg.tx_bps === "number") {
    return `${formatBps(leg.rx_bps)}/${formatBps(leg.tx_bps)}`;
  }
  return "-";
}

function formatLatency(value: number | null | undefined): string {
  return isMeasuredLatency(value) ? `${Math.max(1, Math.round(value))} ms` : "-";
}

function formatBps(value: number | null | undefined): string {
  if (typeof value !== "number") return "-";
  return `${formatBytes(value)}/s`;
}

function formatBytes(value: number | null | undefined): string {
  if (typeof value !== "number") return "-";
  const abs = Math.abs(value);
  if (abs < 1024) return `${Math.round(value)} B`;
  if (abs < 1024 * 1024) return `${(value / 1024).toFixed(abs < 10 * 1024 ? 1 : 0)} KB`;
  return `${(value / (1024 * 1024)).toFixed(abs < 10 * 1024 * 1024 ? 1 : 0)} MB`;
}

export default SessionHealthBar;
