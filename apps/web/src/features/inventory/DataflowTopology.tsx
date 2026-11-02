import { useEffect, useId, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Focus, Globe, Layers, Minus, Plus, Server, Waypoints } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { cn } from "@/shared/lib/utils";
import type { Dataflow } from "./dataflows-api";
import { buildDataflowVisualization, dataflowNeighborhood, fitDataflowGraph, layoutDataflowEdges, layoutDataflowGraph, type FlowGraph, type FlowNode } from "./dataflowVisualization";
import DataflowDrilldown from "./DataflowDrilldown";

export default function DataflowTopology({ flows, graph: suppliedGraph, grouped = false, onInspect }: Readonly<{
  flows: Dataflow[]; graph?: FlowGraph; grouped?: boolean; onInspect: (flow: Dataflow, button: HTMLButtonElement) => void;
}>) {
  const { t } = useTranslation();
  const arrow = useId().replaceAll(":", "");
  const graph = useMemo(() => suppliedGraph ?? buildDataflowVisualization(flows).exact, [suppliedGraph, flows]);
  const [focused, setFocused] = useState<string | null>(null);
  const [edgeKey, setEdgeKey] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const [width, setWidth] = useState(900);
  const canvas = useRef<HTMLElement>(null);
  const drag = useRef<{ x: number; y: number } | null>(null);
  useEffect(() => {
    const element = canvas.current;
    if (!element || typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(entries => { const entry = entries[0]; if (entry) setWidth(Math.round(entry.contentRect.width)); });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const label = (node: FlowNode) => node.grouped || ["all", "internet"].includes(node.endpoint.object_type) ? t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`) : node.endpoint.label ?? t("dataflows.unresolved");
  const neighborhood = !grouped && graph.nodes.length > 12;
  const nearby = useMemo(() => dataflowNeighborhood(graph, focused, page), [graph, focused, page]);
  const active = neighborhood ? nearby.center : graph.nodes.find(node => node.key === focused);
  const nodes = neighborhood ? nearby.nodes : graph.nodes;
  const edges = neighborhood ? nearby.edges : graph.edges;
  const selectedEdge = graph.edges.find(edge => edge.key === edgeKey);
  const selectedFlows = selectedEdge?.flows ?? (active ? graph.edges.filter(edge => edge.source === active.key || edge.destination === active.key).flatMap(edge => edge.flows) : []);
  const layout = layoutDataflowGraph(nodes, width, grouped, neighborhood ? active?.key : undefined);
  const byKey = new Map(graph.nodes.map(node => [node.key, node]));
  const connections = layoutDataflowEdges(edges, layout);
  const reset = () => { setZoom(1); setPan({ x: 0, y: 0 }); setFocused(null); setEdgeKey(null); setPage(0); canvas.current?.scrollTo?.({ top: 0, left: 0 }); };
  const focusNode = (node: FlowNode) => { setFocused(node.key); setEdgeKey(null); setPage(0); setZoom(1); setPan({ x: 0, y: 0 }); canvas.current?.scrollTo?.({ top: 0, left: 0 }); };
  return <section aria-label={t(grouped ? "dataflows.groupedTitle" : "dataflows.topologyTitle")} className="min-w-0 overflow-hidden rounded-lg border border-border bg-card">
    <div className="flex flex-col gap-2 border-b border-border p-4">
      <p className="text-xs text-muted-foreground">{t(grouped ? "dataflows.aggregationHint" : "dataflows.exactGraphHint")}</p>
      <div className="flex flex-wrap items-center gap-2"><span className="text-xs">{t("dataflows.graphCounts", { nodes: graph.nodes.length, edges: graph.edges.length, count: flows.length })}</span>{(["active", "draft", "disabled"] as const).map(status => <span key={status} className={cn("rounded border border-border px-2 py-1 text-xs", status === "active" ? "text-status-positive" : status === "draft" ? "text-status-warning" : "text-muted-foreground")}><span aria-hidden="true">{status === "active" ? "━" : status === "draft" ? "┄" : "╌"} </span>{t(`dataflows.statuses.${status}`)} {flows.filter(flow => flow.status === status).length}</span>)}</div>
      <div className="flex flex-wrap items-center gap-1">
        <Button variant="outline" size="sm" aria-label={t("dataflows.zoomOut")} disabled={zoom <= 0.5} onClick={() => setZoom(value => Math.max(0.5, value - 0.25))}><Minus className="size-4" /></Button><output className="w-12 text-center text-xs">{Math.round(zoom * 100)}%</output><Button variant="outline" size="sm" aria-label={t("dataflows.zoomIn")} disabled={zoom >= 3} onClick={() => setZoom(value => Math.min(3, value + 0.5))}><Plus className="size-4" /></Button>
        <Button variant="outline" size="sm" onClick={() => {
          const element = canvas.current;
          if (!element?.clientWidth || !element.clientHeight) return;
          const fitted = fitDataflowGraph({ width: layout.width, height: connections.height }, { width: element.clientWidth, height: element.clientHeight });
          setZoom(fitted.zoom); setPan({ x: 0, y: 0 });
          element.scrollTo({ left: fitted.left, top: fitted.top });
        }}>{t("dataflows.fitGraph")}</Button><Button variant="outline" size="sm" aria-label={t("dataflows.resetTopology")} onClick={reset}><Focus className="size-4" /></Button>
        {active && !neighborhood && <Button variant="ghost" size="sm" onClick={() => { setFocused(null); setEdgeKey(null); }}>{t("dataflows.clearHighlight")}</Button>}
      </div>
      <Input aria-label={t("dataflows.findNode")} placeholder={t("dataflows.findNode")} value={search} onChange={event => setSearch(event.target.value)} />
      {search && <div className="flex max-h-32 flex-wrap gap-1 overflow-auto">{graph.nodes.filter(node => `${label(node)} ${node.endpoint.object_id ?? ""}`.toLocaleLowerCase().includes(search.toLocaleLowerCase())).slice(0, 30).map(node => <Button key={node.key} size="sm" variant="outline" onClick={() => focusNode(node)}>{label(node)} · {node.endpoint.object_id ?? node.endpoint.object_type}</Button>)}<span className="text-xs text-muted-foreground">{t("dataflows.nodeSearchHint")}</span></div>}
      {neighborhood && <>
        <label className="flex flex-col gap-1 text-xs text-muted-foreground">{t("dataflows.chooseEndpoint")}<select aria-label={t("dataflows.chooseEndpoint")} value={active?.key ?? ""} onChange={event => { const node = byKey.get(event.target.value); if (node) focusNode(node); }} className="control-surface w-full min-w-0 rounded-md p-2 text-sm">{graph.nodes.map(node => <option key={node.key} value={node.key}>{label(node)} · {node.endpoint.object_id ?? node.endpoint.object_type} · {node.endpoint.address_mode}</option>)}</select></label>
        <div className="flex flex-wrap items-center justify-between gap-2"><output className="text-xs text-muted-foreground">{t("dataflows.neighborhoodCounts", { shown: nodes.length, total: graph.nodes.length, edges: edges.length, allEdges: nearby.incident.length, count: edges.reduce((sum, edge) => sum + edge.flows.length, 0) })}</output><div className="flex items-center gap-2"><Button variant="outline" size="sm" disabled={nearby.page === 0} onClick={() => { setPage(nearby.page - 1); setEdgeKey(null); }}>{t("dataflows.previousNeighbors")}</Button><span className="text-xs">{nearby.page + 1} / {nearby.pages}</span><Button variant="outline" size="sm" disabled={nearby.page === nearby.pages - 1} onClick={() => { setPage(nearby.page + 1); setEdgeKey(null); }}>{t("dataflows.nextNeighbors")}</Button></div></div>
      </>}
    </div>
    <section ref={canvas} aria-label={t("dataflows.topologyCanvas")} tabIndex={0} onKeyDown={event => {
      if (event.target !== event.currentTarget) return;
      const moves: Record<string, [number, number]> = { ArrowLeft: [60, 0], ArrowRight: [-60, 0], ArrowUp: [0, 60], ArrowDown: [0, -60] };
      const move = moves[event.key];
      if (move) { event.preventDefault(); const [x, y] = move; setPan(value => ({ x: value.x + x, y: value.y + y })); }
      if (event.key === "+" || event.key === "=") setZoom(value => Math.min(3, value + 0.5));
      if (event.key === "-") setZoom(value => Math.max(0.5, value - 0.25));
      if (event.key === "0") reset();
    }} className="relative max-h-[560px] overflow-auto overscroll-contain bg-muted/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring" onPointerDown={event => {
      if ((event.target as Element).closest("button")) return;
      drag.current = { x: event.clientX, y: event.clientY }; event.currentTarget.setPointerCapture(event.pointerId);
    }} onPointerMove={event => { if (!drag.current) return; setPan(value => ({ x: value.x + (event.clientX - drag.current!.x) / zoom, y: value.y + (event.clientY - drag.current!.y) / zoom })); drag.current = { x: event.clientX, y: event.clientY }; }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }}>
      <svg width={layout.width} height={connections.height} className="block text-foreground">
        <defs><marker id={arrow} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" markerUnits="userSpaceOnUse" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor" /></marker></defs>
        <g transform={`translate(${layout.width / 2} ${connections.height / 2}) scale(${zoom}) translate(${-layout.width / 2 + pan.x} ${-connections.height / 2 + pan.y})`}>
          {connections.edges.map(({ edge, path, connection, badge }) => {
            const bright = !active || edge.source === active.key || edge.destination === active.key;
            const status = edge.flows.every(flow => flow.status === "disabled") ? "disabled" : edge.flows.some(flow => flow.status === "active") ? "active" : "draft";
            return <g key={edge.key} opacity={bright ? 1 : 0.16}>
              <path pointerEvents="none" d={path} fill="none" stroke="currentColor" strokeWidth={grouped ? 2 + Math.min(3, Math.log2(edge.flows.length + 1) / 2) : 2} strokeDasharray={status === "active" ? undefined : status === "draft" ? "2 4" : "7 5"} markerEnd={`url(#${arrow})`} className={status === "active" ? "text-status-positive" : status === "draft" ? "text-status-warning" : "text-muted-foreground"} />
              {(connection.x !== badge.x || connection.y !== badge.y) && <path pointerEvents="none" d={`M ${connection.x} ${connection.y} L ${badge.x} ${badge.y}`} stroke="currentColor" strokeWidth="1" className="text-muted-foreground" />}
              <foreignObject x={badge.x - 30} y={badge.y - 16} width="60" height="32"><button type="button" aria-label={t("dataflows.edgeBucket", { source: label(byKey.get(edge.source)!), destination: label(byKey.get(edge.destination)!), count: edge.flows.length })} aria-pressed={edgeKey === edge.key} onClick={() => setEdgeKey(edge.key)} className="h-8 w-full rounded-full border border-border bg-background text-xs font-semibold text-foreground shadow-sm hover:bg-accent focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">{edge.flows.length} →</button></foreignObject>
            </g>;
          })}
          {nodes.map(node => { const position = layout.positions.get(node.key)!; const Icon = node.endpoint.object_type === "internet" ? Globe : node.grouped ? Layers : node.endpoint.object_type === "dcim.device" || node.endpoint.object_type === "virtualization.virtualmachine" ? Server : Waypoints;
            return <foreignObject key={node.key} x={position.x - 100} y={position.y - 36} width={layout.nodeWidth} height={layout.nodeHeight}><button type="button" aria-pressed={active?.key === node.key} aria-label={t("dataflows.highlightEndpoint", { name: label(node) })} onClick={() => focusNode(node)} title={`${label(node)} · ${node.endpoint.object_id ?? node.endpoint.object_type}`} className={cn("flex size-full items-center gap-2 rounded-lg border bg-card px-3 text-left shadow-sm hover:border-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring", active?.key === node.key ? "border-primary bg-accent" : "border-border")}><Icon aria-hidden="true" className="size-5 shrink-0 text-status-info" /><span className="flex min-w-0 flex-col gap-1"><span className="line-clamp-2 text-sm font-medium leading-4">{label(node)}</span><span className="truncate text-xs text-muted-foreground">{node.grouped ? t("dataflows.selectorCount", { count: node.members.length }) : t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`)}</span></span></button></foreignObject>;
          })}
        </g>
      </svg>
    </section>
    {active && <div className="flex flex-col gap-1 border-t border-border p-4 text-xs"><strong>{label(active)}</strong><span className="break-all text-muted-foreground">{active.endpoint.object_type} · {active.grouped ? t("dataflows.selectorCount", { count: active.members.length }) : `${active.endpoint.object_id ?? "—"} · ${active.endpoint.address_mode ?? "default"} · ${(active.endpoint.ipaddresses ?? []).join(", ")}`}</span>{neighborhood && <span className="text-muted-foreground">{t("dataflows.neighborhoodDrilldown")}</span>}</div>}
    {selectedFlows.length ? <DataflowDrilldown key={selectedEdge?.key ?? active?.key} flows={selectedFlows} onInspect={onInspect} /> : <p className="border-t border-border p-4 text-xs text-muted-foreground">{t("dataflows.selectBucket")}</p>}
  </section>;
}
