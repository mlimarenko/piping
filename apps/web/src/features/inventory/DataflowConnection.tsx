import type { SVGProps } from "react";
import { useTranslation } from "react-i18next";
import { Globe, Layers, Network, Server, Users } from "lucide-react";
import { badgeVariants } from "@/shared/ui-primitives/badge";
import { cn } from "@/shared/lib/utils";
import type { Dataflow, DataflowEndpointResponse, DataflowTransport } from "./dataflows-api";
import { DATAFLOW_ENDPOINTS } from "./dataflowEndpoints";
import { aggregateFlowStatuses, FLOW_STATUSES } from "./dataflowVisualization";

const ENDPOINT_ICONS = { internet: Globe, all: Network, group: Users, host: Server, service: Layers, address: Layers, range: Layers, tag: Layers, addressgroup: Layers };
const STATUS_STYLES = {
  active: { badge: "status-active", line: "text-status-positive", dash: undefined },
  draft: { badge: "status-planned", line: "text-status-info", dash: "2 4" },
  disabled: { badge: "status-neutral", line: "text-muted-foreground", dash: "7 5" },
} as const;
const MIXED_LINE = { line: "text-muted-foreground", dash: "7 3 2 3" } as const;

export function endpointTypeLabel(endpoint: DataflowEndpointResponse, t: (key: string, options?: Record<string, unknown>) => string): string {
  const type = t(`dataflows.endpointTypes.${endpoint.object_type.replaceAll(".", "_")}`, {
    defaultValue: DATAFLOW_ENDPOINTS[endpoint.object_type].entity ?? t("dataflows.unresolved"),
  });
  if (endpoint.object_type !== "extras.tag" || !endpoint.tag_object_type) return type;
  return `${type} · ${t(endpoint.tag_object_type === "dcim.device" ? "dataflows.tagScopeDevice" : "dataflows.tagScopeVm")}`;
}

export function endpointDisplayName(endpoint: DataflowEndpointResponse, t: (key: string, options?: Record<string, unknown>) => string): string {
  const kind = DATAFLOW_ENDPOINTS[endpoint.object_type].kind;
  const type = endpointTypeLabel(endpoint, t);
  if (kind === "internet" || kind === "all") return type;
  const label = endpoint.label ?? t("dataflows.unresolved");
  if (endpoint.object_type !== "extras.tag" || !endpoint.tag_object_type) return label;
  return `${label} · ${t(endpoint.tag_object_type === "dcim.device" ? "dataflows.tagScopeDevice" : "dataflows.tagScopeVm")}`;
}

/** Bounded readable side summary: the first name plus the hidden count. */
export function endpointSummaryText(endpoints: readonly DataflowEndpointResponse[], t: (key: string, options?: Record<string, unknown>) => string): string {
  const [first, ...rest] = endpoints;
  if (!first) return t("dataflows.unresolved");
  const name = endpointDisplayName(first, t);
  return rest.length > 0 ? `${name} +${rest.length}` : name;
}

export function transportLabel(transport: DataflowTransport, t: (key: string, options?: Record<string, unknown>) => string): string {
  if (transport === "any") return t("dataflows.anyTransport");
  if (transport === "tcp_udp") return t("dataflows.transportTcpUdp");
  if (transport === "icmp") return t("dataflows.transportIcmp");
  if (transport === "icmpv6") return t("dataflows.transportIcmpv6");
  return transport.toUpperCase();
}

export function FlowStatus({ flow, title }: Readonly<{ flow: Dataflow; title?: string }>) {
  const { t } = useTranslation();
  return <span title={title} className={cn(badgeVariants({ variant: STATUS_STYLES[flow.effective_status].badge }), "w-fit self-center bg-card")}>{t(`dataflows.statuses.${flow.effective_status}`)}</span>;
}

export function FlowStatusLine({ status, className, ...props }: Readonly<SVGProps<SVGPathElement> & { status: Dataflow["effective_status"] | "mixed" }>) {
  const style = status === "mixed" ? MIXED_LINE : STATUS_STYLES[status];
  return <path {...props} fill="none" stroke="currentColor" strokeDasharray={style.dash} className={cn(style.line, className)} />;
}

export function FlowStatusSummary({ flows }: Readonly<{ flows: Dataflow[] }>) {
  const { t } = useTranslation();
  const { counts } = aggregateFlowStatuses(flows);
  return <dl aria-label={t("dataflows.status")} className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground">
    {FLOW_STATUSES.map(status => <div key={status} className="inline-flex items-center gap-1.5">
      <dt className="inline-flex items-center gap-1.5">
        <svg aria-hidden="true" focusable="false" viewBox="0 0 24 12" className="h-3 w-5 shrink-0"><FlowStatusLine status={status} d="M 0 6 H 24" strokeWidth={2} /></svg>
        {t(`dataflows.statuses.${status}`)}
      </dt>
      <dd className="font-medium tabular-nums text-foreground">{counts[status]}</dd>
    </div>)}
    <div className="inline-flex items-center gap-1.5"><dt className="inline-flex items-center gap-1.5">
      <svg aria-hidden="true" focusable="false" viewBox="0 0 24 12" className="h-3 w-5 shrink-0"><FlowStatusLine status="mixed" d="M 0 6 H 24" strokeWidth={2} /></svg>
      {t("dataflows.mixedStatuses")}
    </dt><dd aria-hidden="true" /></div>
  </dl>;
}

function compactList(values: readonly number[], compact: boolean) {
  if (compact && values.length > 3) return null;
  return values.join(", ");
}

export function FlowTraffic({ flow, compact = false }: Readonly<{ flow: Dataflow; compact?: boolean }>) {
  const { t } = useTranslation();
  const icmp = flow.transport === "icmp" || flow.transport === "icmpv6";
  const destinationPorts = flow.destination_ports ?? [];
  const sourcePorts = flow.source_ports ?? [];
  const icmpTypes = flow.icmp_types ?? [];
  return <span className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-xs">
    <span className="font-semibold text-foreground">{transportLabel(flow.transport, t)}</span>
    {icmp ? icmpTypes.length > 0 && <span className="break-words font-mono text-[11px] text-muted-foreground">
      {t("dataflows.icmpTypes")}: {compactList(icmpTypes, compact) ?? t("dataflows.portCount", { count: icmpTypes.length })}
    </span> : <>
      <span className="break-words font-mono text-[11px] text-muted-foreground">{destinationPorts.length > 0 ? compactList(destinationPorts, compact) ?? t("dataflows.portCount", { count: destinationPorts.length }) : t("dataflows.anyPorts")}</span>
      {sourcePorts.length > 0 && <span className="break-words font-mono text-[11px] text-muted-foreground">
        {t("dataflows.sourcePorts")}: {compactList(sourcePorts, compact) ?? t("dataflows.portCount", { count: sourcePorts.length })}
      </span>}
    </>}
  </span>;
}

/** First readable endpoint plus a bounded remainder; the full list lives in detail. */
export function EndpointSummary({ endpoints }: Readonly<{ endpoints: readonly DataflowEndpointResponse[] }>) {
  const { t } = useTranslation();
  const names = endpoints.map(endpoint => endpointDisplayName(endpoint, t));
  return <span title={names.join(", ")} className="min-w-0 break-words text-sm">{endpointSummaryText(endpoints, t)}</span>;
}

export function EndpointLabel({ endpoint, side }: Readonly<{ endpoint: DataflowEndpointResponse; side: string }>) {
  const { t } = useTranslation();
  const kind = DATAFLOW_ENDPOINTS[endpoint.object_type].kind;
  const Icon = ENDPOINT_ICONS[kind];
  const type = endpointTypeLabel(endpoint, t);
  const label = kind === "internet" || kind === "all" ? type : endpointDisplayName(endpoint, t);
  return <span className="flex min-w-0 items-start gap-2 text-foreground">
    <Icon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
    <span className="sr-only">{side} · {type}</span>
    <span title={label} className="min-w-0 text-sm font-medium [overflow-wrap:anywhere]">{label}</span>
  </span>;
}
