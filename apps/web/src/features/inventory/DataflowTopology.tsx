import { useCallback, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeft, Focus, Globe, Layers, Maximize2, Minus, Plus, Server, Waypoints } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import SurfaceCard, { SurfaceCardHeader } from "@/shared/ui-shared/SurfaceCard";
import { cn } from "@/shared/lib/utils";
import type { Dataflow } from "./dataflows-api";
import { aggregateFlowStatuses, buildDataflowVisualization, dataflowRelations, fitDataflowGraph, layoutDataflowTopology, type FlowEdge, type FlowNode } from "./dataflowVisualization";
import DataflowDrilldown from "./DataflowDrilldown";
import { FlowStatusLine } from "./DataflowConnection";

const MIN_ZOOM = 0.5;
const MAX_ZOOM = 3;
const SEARCH_LIMIT = 50;
const PAN_STEP = 60;

type EdgeCount = { key: string; edge: FlowEdge; source: string; destination: string; active: boolean };

function endpointIcon(node: FlowNode) {
  if (node.endpoint.object_type === "internet") return Globe;
  if (node.endpoint.object_type === "all") return Layers;
  if (node.endpoint.object_type === "dcim.device" || node.endpoint.object_type === "virtualization.virtualmachine") return Server;
  return Waypoints;
}

function TopologyNodeCard({ node, name, subtitle, selected, counts, onSelect, onCount, className }: Readonly<{
  node: FlowNode;
  name: string;
  subtitle: string;
  selected: boolean;
  counts: EdgeCount[];
  onSelect: (node: FlowNode) => void;
  onCount: (edge: FlowEdge) => void;
  className?: string;
}>) {
  const { t } = useTranslation();
  const Icon = endpointIcon(node);
  return <div className={cn("flex min-w-0 items-stretch gap-1 rounded-lg border bg-card p-1.5 shadow-sm", selected ? "border-primary bg-accent" : "border-border", className)}>
    <button type="button" aria-pressed={selected} aria-label={t("dataflows.highlightEndpoint", { name })} title={`${name} · ${node.endpoint.object_id ?? node.endpoint.object_type}`} onClick={() => onSelect(node)} className="flex min-w-0 flex-1 items-start gap-2 rounded-md px-1 py-1 text-left hover:bg-accent/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
      <Icon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-status-info" />
      <span data-node-content className="flex min-w-0 flex-col gap-0.5">
        <span className="break-words text-sm font-medium leading-4 [overflow-wrap:anywhere]">{name}</span>
        <span className="break-words text-xs text-muted-foreground [overflow-wrap:anywhere]">{subtitle}</span>
      </span>
    </button>
    {counts.map(count => <button key={count.key} type="button" aria-pressed={count.active} aria-label={t("dataflows.edgeBucket", { source: count.source, destination: count.destination, count: count.edge.flows.length })} onClick={() => onCount(count.edge)} className={cn("w-9 shrink-0 self-center rounded-full border px-1 py-0.5 text-xs font-semibold tabular-nums focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring", count.active ? "border-primary bg-primary text-primary-foreground" : "border-border bg-background text-foreground hover:bg-accent")}>{count.edge.flows.length}</button>)}
  </div>;
}

function InternalFlowControl({ counts, name, onCount }: Readonly<{ counts: EdgeCount[]; name: string; onCount: (edge: FlowEdge) => void }>) {
  const { t } = useTranslation();
  return <div className="flex h-full min-w-0 items-center justify-between gap-2 rounded-lg border border-border bg-card px-2 shadow-sm">
    <span className="truncate text-xs font-medium text-muted-foreground">{t("dataflows.internalFlow")}</span>
    {counts.map(count => <button key={count.key} type="button" aria-pressed={count.active} aria-label={t("dataflows.internalFlowLabel", { name, count: count.edge.flows.length })} onClick={() => onCount(count.edge)} className={cn("min-h-6 min-w-9 shrink-0 rounded-full border px-2 text-xs font-semibold tabular-nums focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring", count.active ? "border-primary bg-primary text-primary-foreground" : "border-border bg-background text-foreground hover:bg-accent")}>{count.edge.flows.length}</button>)}
  </div>;
}

export default function DataflowTopology({ flows, onInspect }: Readonly<{
  flows: Dataflow[]; onInspect: (flow: Dataflow, button: HTMLButtonElement) => void;
}>) {
  const { t } = useTranslation();
  const arrow = useId().replaceAll(":", "");
  const graph = useMemo(() => buildDataflowVisualization(flows).exact, [flows]);
  const [focused, setFocused] = useState<string | null>(null);
  const [page, setPage] = useState(0);
  const [search, setSearch] = useState("");
  const [edgeKey, setEdgeKey] = useState<string | null>(null);
  const [zoom, setZoom] = useState(1);
  const [width, setWidth] = useState(900);
  const [nodeHeight, setNodeHeight] = useState(72);
  const [fitRequest, setFitRequest] = useState<{ left: number; top: number } | null>(null);
  const canvas = useRef<HTMLElement>(null);
  const drag = useRef<{ x: number; y: number; left: number; top: number } | null>(null);
  const relations = useMemo(() => dataflowRelations(graph, focused, page), [graph, focused, page]);
  const layout = layoutDataflowTopology(relations, width, nodeHeight);
  const byKey = useMemo(() => new Map(graph.nodes.map(node => [node.key, node])), [graph.nodes]);
  const label = useCallback((node: FlowNode) => node.endpoint.object_type === "all" || node.endpoint.object_type === "internet"
    ? t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`)
    : node.endpoint.label ?? t("dataflows.unresolved"), [t]);
  const subtitle = useCallback((node: FlowNode) => t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`), [t]);
  const nodeName = (key: string) => { const node = byKey.get(key); return node ? label(node) : t("dataflows.unresolved"); };
  const countFor = (edge: FlowEdge): EdgeCount => ({
    key: edge.key,
    edge,
    source: nodeName(edge.source),
    destination: nodeName(edge.destination),
    active: edgeKey === edge.key,
  });
  const matches = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    if (!query) return [];
    return graph.nodes.filter(node => `${label(node)} ${subtitle(node)} ${node.endpoint.object_id ?? ""} ${node.endpoint.object_type}`.toLocaleLowerCase().includes(query));
  }, [graph.nodes, label, search, subtitle]);
  const visibleMatches = matches.slice(0, SEARCH_LIMIT);
  const shown = new Set([
    ...(relations.center ? [relations.center.key] : []),
    ...relations.incoming.map(item => item.node.key),
    ...relations.outgoing.map(item => item.node.key),
  ]).size;
  const selectedEdge = relations.incidentEdges.find(edge => edge.key === edgeKey);
  const selectedFlows = selectedEdge?.flows ?? relations.incidentFlows;
  const laneItems = [...layout.incoming, ...layout.outgoing];

  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element) return;
    const measure = () => {
      const next = Math.round(element.clientWidth);
      if (next > 0) setWidth(previous => previous === next ? previous : next);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(entries => {
      const entry = entries[0];
      const next = entry ? Math.round(entry.contentRect.width) : 0;
      if (next > 0) setWidth(previous => previous === next ? previous : next);
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useLayoutEffect(() => {
    const contents = canvas.current?.querySelectorAll<HTMLElement>("[data-node-content]");
    const measured = contents ? Array.from(contents) : [];
    if (!measured.length) { setNodeHeight(previous => previous === 72 ? previous : 72); return; }
    const measure = () => {
      const height = Math.max(72, ...measured.map(element => element.scrollHeight + 24));
      setNodeHeight(previous => previous === height ? previous : height);
    };
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    measured.forEach(element => observer.observe(element));
    return () => observer.disconnect();
  }, [relations, width, layout.mode]);

  // Auto-fit runs for a new centre, page, width or measured height; manual zoom and pan never re-trigger it.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  useLayoutEffect(() => {
    const element = canvas.current;
    if (!element || !element.clientWidth || !element.clientHeight) return;
    if (layout.mode === "lanes" && (!layout.width || !layout.height)) return;
    const fitted = fitDataflowGraph(layout, { width: element.clientWidth, height: element.clientHeight });
    setZoom(fitted.zoom);
    setFitRequest({ left: fitted.left, top: fitted.top });
  }, [relations.center?.key, relations.page, width, nodeHeight, layout.mode]);

  useLayoutEffect(() => {
    if (!fitRequest) return;
    const element = canvas.current;
    if (!element) return;
    if (typeof element.scrollTo === "function") element.scrollTo({ left: fitRequest.left, top: fitRequest.top });
    else {
      element.scrollLeft = fitRequest.left;
      element.scrollTop = fitRequest.top;
    }
  }, [fitRequest]);

  const selectNode = (node: FlowNode) => { setFocused(node.key); setPage(0); setEdgeKey(null); setSearch(""); };
  const changePage = (next: number) => { setPage(next); setEdgeKey(null); };
  const setBoundedZoom = (next: number) => setZoom(Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, next)));
  const applyFit = () => {
    const element = canvas.current;
    if (!element || !element.clientWidth || !element.clientHeight) return;
    const fitted = fitDataflowGraph(layout, { width: element.clientWidth, height: element.clientHeight });
    setZoom(fitted.zoom);
    setFitRequest({ left: fitted.left, top: fitted.top });
  };
  const reset = () => {
    setFocused(null);
    setPage(0);
    setSearch("");
    setEdgeKey(null);
    setZoom(1);
    const element = canvas.current;
    if (!element) return;
    if (typeof element.scrollTo === "function") element.scrollTo({ left: 0, top: 0 });
    else {
      element.scrollLeft = 0;
      element.scrollTop = 0;
    }
  };
  const onKeyDown = (event: KeyboardEvent<HTMLElement>) => {
    if (event.target !== event.currentTarget) return;
    if (event.key === "ArrowLeft") { event.preventDefault(); event.currentTarget.scrollLeft -= PAN_STEP; return; }
    if (event.key === "ArrowRight") { event.preventDefault(); event.currentTarget.scrollLeft += PAN_STEP; return; }
    if (event.key === "ArrowUp") { event.preventDefault(); event.currentTarget.scrollTop -= PAN_STEP; return; }
    if (event.key === "ArrowDown") { event.preventDefault(); event.currentTarget.scrollTop += PAN_STEP; return; }
    if (event.key === "+" || event.key === "=") { event.preventDefault(); setBoundedZoom(zoom + 0.5); return; }
    if (event.key === "-") { event.preventDefault(); setBoundedZoom(zoom - 0.25); return; }
    if (event.key === "0") { event.preventDefault(); reset(); }
  };

  const countCard = (node: FlowNode, edge: FlowEdge, className?: string) => <TopologyNodeCard node={node} name={label(node)} subtitle={subtitle(node)} selected={relations.center?.key === node.key} counts={[countFor(edge)]} onSelect={selectNode} onCount={selected => setEdgeKey(selected.key)} className={className} />;
  const centerCard = (className?: string) => relations.center ? <TopologyNodeCard node={relations.center} name={label(relations.center)} subtitle={subtitle(relations.center)} selected counts={[]} onSelect={selectNode} onCount={edge => setEdgeKey(edge.key)} className={className} /> : null;
  const selfCards = relations.selfEdges.map(edge => countFor(edge));

  return <SurfaceCard role="region" aria-label={t("dataflows.topologyTitle")} className="overflow-hidden">
    <SurfaceCardHeader className="gap-3 p-3 sm:p-4">
      <div className="flex min-w-0 flex-col gap-2">
        <div className="flex min-w-0 flex-wrap items-center gap-2">
          <Input className="min-w-0 flex-1 basis-40 sm:flex-none sm:w-64 sm:grow-0 sm:basis-64" aria-label={t("dataflows.findNode")} placeholder={t("dataflows.findNode")} value={search} onChange={event => setSearch(event.target.value)} />
          <span className="hidden text-xs text-muted-foreground sm:inline">{t("dataflows.graphNodeEdgeCounts", { nodes: graph.nodes.length, edges: graph.edges.length })}</span>
          <span className="text-xs text-muted-foreground sm:hidden">{t("dataflows.neighborhoodCountsCompact", { shown, total: graph.nodes.length, count: relations.incidentFlows.length })}</span>
          <details className="relative ml-auto shrink-0 text-xs text-muted-foreground">
            <summary aria-label={t("dataflows.graphHelp")} className="cursor-pointer list-none rounded px-2 py-1.5 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring [&::-webkit-details-marker]:hidden">
              <span aria-hidden="true" className="sm:hidden">?</span><span className="hidden sm:inline">{t("dataflows.graphHelp")}</span>
            </summary>
            <p className="absolute right-0 top-full z-20 w-64 rounded-md border border-border bg-popover p-3 text-xs text-popover-foreground shadow-md">{t("dataflows.exactGraphHint")}</p>
          </details>
        </div>
        {search.trim() && <div className="flex flex-col gap-1">
          <ul aria-label={t("dataflows.findNode")} className="flex max-h-40 flex-wrap gap-1 overflow-auto">
            {visibleMatches.map(node => <li key={node.key}><Button variant="outline" size="sm" onClick={() => selectNode(node)}>{label(node)} · {node.endpoint.object_id ?? node.endpoint.object_type}</Button></li>)}
          </ul>
          {matches.length > SEARCH_LIMIT && <p className="text-xs text-muted-foreground">{t("dataflows.topologySearchMore", { shown: visibleMatches.length, total: matches.length })}</p>}
        </div>}
        <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
          <span className="hidden min-w-0 text-xs text-muted-foreground sm:block sm:flex-1">{t("dataflows.neighborhoodCounts", { shown, total: graph.nodes.length, edges: relations.visibleEdges.length, allEdges: relations.incidentEdges.length, count: relations.incidentFlows.length })}</span>
          <div className="flex min-w-0 flex-1 items-center justify-between gap-2 sm:flex-none sm:justify-end">
            <div className="flex min-w-0 items-center gap-1">
              <Button variant="outline" size="sm" className="px-2" aria-label={t("dataflows.previousNeighbors")} disabled={relations.page === 0} onClick={() => changePage(relations.page - 1)}>
                <ChevronLeft aria-hidden="true" className="size-4 md:hidden" /><span className="hidden md:inline">{t("dataflows.previousNeighbors")}</span>
              </Button>
              <span className="whitespace-nowrap text-xs tabular-nums">{relations.page + 1} / {relations.pages}</span>
              <Button variant="outline" size="sm" disabled={relations.page >= relations.pages - 1} onClick={() => changePage(relations.page + 1)}>{t("dataflows.nextNeighbors")}</Button>
            </div>
            <div className="flex shrink-0 items-center gap-1">
              <Button variant="outline" size="sm" className="px-2" aria-label={t("dataflows.zoomOut")} disabled={zoom <= MIN_ZOOM} onClick={() => setBoundedZoom(zoom - 0.25)}><Minus className="size-4" /></Button>
              <output className="hidden w-12 text-center text-xs tabular-nums sm:inline">{Math.round(zoom * 100)}%</output>
              <Button variant="outline" size="sm" className="px-2" aria-label={t("dataflows.zoomIn")} disabled={zoom >= MAX_ZOOM} onClick={() => setBoundedZoom(zoom + 0.5)}><Plus className="size-4" /></Button>
              <Button variant="outline" size="sm" className="px-2" aria-label={t("dataflows.fitGraph")} onClick={applyFit}><Maximize2 aria-hidden="true" className="size-4 sm:hidden" /><span className="hidden sm:inline">{t("dataflows.fitGraph")}</span></Button>
              <Button variant="outline" size="sm" className="px-2" aria-label={t("dataflows.resetTopology")} onClick={reset}><Focus className="size-4" /></Button>
            </div>
          </div>
        </div>
      </div>
    </SurfaceCardHeader>
    <section ref={canvas} aria-label={t("dataflows.topologyCanvas")} tabIndex={0} onKeyDown={onKeyDown} onPointerDown={event => {
      if ((event.target as Element).closest("button")) return;
      const element = event.currentTarget;
      drag.current = { x: event.clientX, y: event.clientY, left: element.scrollLeft, top: element.scrollTop };
      element.setPointerCapture?.(event.pointerId);
    }} onPointerMove={event => {
      const start = drag.current;
      if (!start) return;
      event.currentTarget.scrollLeft = start.left - (event.clientX - start.x);
      event.currentTarget.scrollTop = start.top - (event.clientY - start.y);
    }} onPointerUp={() => { drag.current = null; }} onPointerCancel={() => { drag.current = null; }} className="relative max-h-[560px] overflow-auto overscroll-contain bg-muted/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring">
      {layout.mode === "lanes" ? <svg width={layout.width * zoom} height={layout.height * zoom} className="mx-auto block text-foreground">
        <defs><marker id={arrow} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" markerUnits="userSpaceOnUse" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="currentColor" /></marker></defs>
        <g transform={`scale(${zoom})`}>
          {laneItems.map(item => {
            const edge = item.neighbor.edge;
            const { status } = aggregateFlowStatuses(edge.flows);
            return <FlowStatusLine key={item.key} status={status} pointerEvents="none" d={item.path} strokeWidth={2} markerEnd={`url(#${arrow})`} />;
          })}
          <text x={layout.headers.incoming.x} y={layout.headers.incoming.y} textAnchor="middle" fill="currentColor" className="text-muted-foreground text-xs font-medium">{t("dataflows.incomingTitle")}</text>
          <text x={layout.headers.outgoing.x} y={layout.headers.outgoing.y} textAnchor="middle" fill="currentColor" className="text-muted-foreground text-xs font-medium">{t("dataflows.outgoingTitle")}</text>
          {layout.incoming.map(item => <foreignObject key={item.key} x={item.x - item.width / 2} y={item.y - item.height / 2} width={item.width} height={item.height}>{countCard(item.neighbor.node, item.neighbor.edge, "size-full")}</foreignObject>)}
          {layout.outgoing.map(item => <foreignObject key={item.key} x={item.x - item.width / 2} y={item.y - item.height / 2} width={item.width} height={item.height}>{countCard(item.neighbor.node, item.neighbor.edge, "size-full")}</foreignObject>)}
          {relations.center && layout.center && <foreignObject x={layout.center.x - layout.center.width / 2} y={layout.center.y - layout.center.height / 2} width={layout.center.width} height={layout.center.height}>{centerCard("size-full")}</foreignObject>}
          {layout.self && relations.selfEdges.length > 0 && <foreignObject overflow="visible" x={layout.self.x - layout.self.width / 2} y={layout.self.y - layout.self.height / 2} width={layout.self.width} height={layout.self.height}><InternalFlowControl counts={selfCards} name={relations.center ? label(relations.center) : ""} onCount={edge => setEdgeKey(edge.key)} /></foreignObject>}
        </g>
      </svg> : <div className="flex flex-col gap-3 p-3">
        {centerCard("w-full min-h-[72px]")}
        {relations.incoming.length > 0 && <section className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{t("dataflows.incomingTitle")}</h3>
          <ul className="flex flex-col gap-2">{relations.incoming.map(item => <li key={item.key}>{countCard(item.node, item.edge, "w-full")}</li>)}</ul>
        </section>}
        {relations.outgoing.length > 0 && <section className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{t("dataflows.outgoingTitle")}</h3>
          <ul className="flex flex-col gap-2">{relations.outgoing.map(item => <li key={item.key}>{countCard(item.node, item.edge, "w-full")}</li>)}</ul>
        </section>}
        {relations.selfEdges.length > 0 && <section className="flex flex-col gap-2">
          <h3 className="text-sm font-medium">{t("dataflows.internalFlow")}</h3>
          <ul className="flex flex-col gap-2"><li><InternalFlowControl counts={selfCards} name={relations.center ? label(relations.center) : ""} onCount={edge => setEdgeKey(edge.key)} /></li></ul>
        </section>}
      </div>}
    </section>
    {selectedFlows.length ? <DataflowDrilldown key={selectedEdge?.key ?? relations.center?.key ?? "empty"} flows={selectedFlows} onInspect={onInspect} /> : <p className="border-t border-border p-4 text-xs text-muted-foreground">{t("dataflows.selectBucket")}</p>}
  </SurfaceCard>;
}
