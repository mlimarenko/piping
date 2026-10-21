import { useId, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Focus, Minus, Plus } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";
import type { Dataflow } from "./dataflows-api";
import { dataflowEndpointKey } from "./dataflowEndpoints";
import { EndpointLabel, FlowStatus, FlowTraffic } from "./DataflowConnection";

// A lane per connection keeps parallel, cyclic and self-directed flows selectable.
// Nodes represent selectors, never expanded or inferred inventory membership.
function layout(flows: Dataflow[]) {
  const endpoints = new Map<string, Dataflow["source"]>();
  for (const flow of flows) for (const endpoint of [flow.source, flow.destination]) endpoints.set(dataflowEndpointKey(endpoint), endpoint);
  let left = 0, right = 0;
  const nodes = [...endpoints].map(([key, endpoint], index) => ({ key, endpoint, x: index % 2 === 0 ? 32 : 908, y: 48 + (index % 2 === 0 ? left++ : right++) * 160 }));
  return { nodes, byKey: new Map(nodes.map(node => [node.key, node])), height: Math.max(420, Math.max(left, right) * 160 + 80, flows.length * 88 + 80) };
}

export default function DataflowTopology({ flows, onInspect }: Readonly<{
  flows: Dataflow[]; onInspect: (flow: Dataflow, button: HTMLButtonElement) => void;
}>) {
  const { t } = useTranslation();
  const arrow = useId().replaceAll(":", "");
  const [focused, setFocused] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const graph = useMemo(() => layout(flows), [flows]);
  const activeFocus = focused && graph.byKey.has(focused) ? focused : null;
  const related = new Set<string>();
  for (const flow of flows) if (!activeFocus || [dataflowEndpointKey(flow.source), dataflowEndpointKey(flow.destination)].includes(activeFocus)) {
    related.add(dataflowEndpointKey(flow.source)); related.add(dataflowEndpointKey(flow.destination));
  }
  return <section aria-label={t("dataflows.topologyTitle")} className="overflow-hidden rounded-lg border border-border bg-card">
    <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border px-4 py-2">
      <p className="max-w-2xl text-xs text-muted-foreground">{t("dataflows.topologyHint")}</p>
      <div className="flex items-center gap-1">
        {activeFocus && <Button variant="ghost" size="sm" onClick={() => setFocused(null)}>{t("dataflows.clearHighlight")}</Button>}
        <Button variant="ghost" size="icon" aria-label={t("dataflows.zoomOut")} disabled={zoom <= 0.5} onClick={() => setZoom(value => Math.max(0.5, value - 0.25))}><Minus className="size-4" /></Button>
        <output className="w-12 text-center text-xs tabular-nums text-muted-foreground">{Math.round(zoom * 100)}%</output>
        <Button variant="ghost" size="icon" aria-label={t("dataflows.zoomIn")} disabled={zoom >= 1.5} onClick={() => setZoom(value => Math.min(1.5, value + 0.25))}><Plus className="size-4" /></Button>
        <Button variant="ghost" size="icon" aria-label={t("dataflows.resetTopology")} onClick={() => { setZoom(1); setFocused(null); }}><Focus className="size-4" /></Button>
      </div>
    </div>
    <section aria-label={t("dataflows.topologyCanvas")} tabIndex={0} className="max-h-[65dvh] overflow-auto overscroll-contain bg-muted/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
      <svg width={1180 * zoom} height={graph.height * zoom} viewBox={`0 0 1180 ${graph.height}`} className="block text-foreground">
        <defs><marker id={arrow} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor" /></marker></defs>
        {flows.map((flow, index) => {
          const source = graph.byKey.get(dataflowEndpointKey(flow.source))!;
          const destination = graph.byKey.get(dataflowEndpointKey(flow.destination))!;
          const y = 78 + index * 88;
          const bright = !activeFocus || source.key === activeFocus || destination.key === activeFocus;
          const endX = destination.x === 32 ? destination.x + 240 : destination.x;
          const startX = source.x === 32 ? source.x + 240 : source.x;
          const sourceLane = source.x === 32 ? 360 : 820;
          const destinationLane = destination.x === 32 ? 360 : 820;
          const cardEntry = source.x === 32 ? 460 : 720;
          const cardExit = destination.x === 32 ? 460 : 720;
          return <g key={flow.id} className={cn("transition-opacity", !bright && "opacity-20")}>
            <path d={`M ${startX} ${source.y + 40} C ${sourceLane} ${source.y + 40}, ${sourceLane} ${y - 8}, ${cardEntry} ${y - 8} M ${cardExit} ${y + 8} C ${destinationLane} ${y + 8}, ${destinationLane} ${destination.y + 40}, ${endX} ${destination.y + 40}`} fill="none" stroke="currentColor" strokeWidth={bright && activeFocus ? 2 : 1.5} strokeDasharray={flow.status === "disabled" ? "5 5" : undefined} markerEnd={`url(#${arrow})`} className={flow.status === "active" ? "text-primary" : "text-muted-foreground"}><title>{flow.name}</title></path>
            <foreignObject x="460" y={y - 32} width="260" height="72">
              <button type="button" aria-haspopup="dialog" aria-label={t("dataflows.inspectFlow", { name: flow.name })} onClick={event => onInspect(flow, event.currentTarget)} className="flex h-16 w-full flex-col justify-center gap-1 rounded-lg border border-border bg-card px-3 text-left shadow-sm transition-colors hover:border-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
                <span title={flow.name} className="truncate text-xs font-medium">{flow.name}</span>
                <span className="flex items-center justify-between gap-2"><FlowTraffic flow={flow} compact /><FlowStatus flow={flow} /></span>
              </button>
            </foreignObject>
          </g>;
        })}
        {graph.nodes.map(node => <foreignObject key={node.key} x={node.x} y={node.y} width="240" height="88" className={cn("transition-opacity", !related.has(node.key) && "opacity-30")}>
          <button type="button" aria-pressed={activeFocus === node.key} aria-label={t("dataflows.highlightEndpoint", { name: node.endpoint.label ?? t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`) })} onClick={() => setFocused(value => value === node.key ? null : node.key)} className={cn("flex h-20 w-full flex-col justify-center gap-2 rounded-lg border bg-card px-4 text-left shadow-sm transition-colors hover:border-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring", activeFocus === node.key ? "border-primary bg-accent" : "border-border")}>
            <EndpointLabel endpoint={node.endpoint} side={t("dataflows.endpoint")} />
            <span className="flex items-center justify-between gap-2 text-[11px] text-muted-foreground"><span>{t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`)}</span><span>{t(`dataflows.addressModes.${node.endpoint.address_mode}`)}</span></span>
          </button>
        </foreignObject>)}
      </svg>
    </section>
  </section>;
}
