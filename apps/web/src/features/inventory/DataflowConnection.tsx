import { useTranslation } from "react-i18next";
import { Globe, Layers, Network, Server, Users } from "lucide-react";
import { badgeVariants } from "@/shared/ui-primitives/badge";
import { cn } from "@/shared/lib/utils";
import type { Dataflow } from "./dataflows-api";
import { DATAFLOW_ENDPOINTS } from "./dataflowEndpoints";

const ENDPOINT_ICONS = { internet: Globe, all: Network, group: Users, host: Server, service: Layers, address: Layers };
const STATUS_VARIANTS = { active: "status-active", draft: "status-planned", disabled: "status-neutral" } as const;

export function FlowStatus({ flow }: Readonly<{ flow: Dataflow }>) {
  const { t } = useTranslation();
  return <span className={cn(badgeVariants({ variant: STATUS_VARIANTS[flow.status] }), "w-fit self-center bg-card")}>{t(`dataflows.statuses.${flow.status}`)}</span>;
}

export function FlowTraffic({ flow, compact = false }: Readonly<{ flow: Dataflow; compact?: boolean }>) {
  const { t } = useTranslation();
  const ports = compact && (flow.destination_ports?.length ?? 0) > 3
    ? t("dataflows.portCount", { count: flow.destination_ports!.length })
    : flow.destination_ports?.join(", ") ?? t("dataflows.anyPorts");
  return <span className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-xs">
    <span className="font-semibold text-foreground">{flow.transport === "any" ? t("dataflows.anyTransport") : flow.transport.toUpperCase()}</span>
    <span className="break-words font-mono text-[11px] text-muted-foreground">{ports}</span>
  </span>;
}

export function EndpointLabel({ endpoint, side }: Readonly<{ endpoint: Dataflow["source"]; side: string }>) {
  const { t } = useTranslation();
  const kind = DATAFLOW_ENDPOINTS[endpoint.object_type].kind;
  const Icon = ENDPOINT_ICONS[kind];
  const type = t(`dataflows.endpointTypes.${endpoint.object_type.replaceAll(".", "_")}`);
  const label = kind === "internet" || kind === "all" ? type : endpoint.label ?? t("dataflows.unresolved");
  return <span className="flex min-w-0 items-center gap-2 text-foreground">
    <Icon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
    <span className="sr-only">{side} · {type}</span>
    <span title={label} className="min-w-0 break-all text-sm font-medium md:truncate">{label}</span>
  </span>;
}
