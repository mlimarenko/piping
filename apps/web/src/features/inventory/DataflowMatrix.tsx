import { useState } from "react";
import { useTranslation } from "react-i18next";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import type { Dataflow } from "./dataflows-api";
import type { FlowGraph, FlowNode } from "./dataflowVisualization";
import DataflowDrilldown from "./DataflowDrilldown";

export default function DataflowMatrix({ graph, onInspect }: Readonly<{ graph: FlowGraph; onInspect: (flow: Dataflow, button: HTMLButtonElement) => void }>) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<string | null>(null);
  const edge = graph.edges.find(item => item.key === selected);
  const cells = new Map(graph.edges.map(item => [item.key, item]));
  const max = Math.max(1, ...graph.edges.map(item => item.flows.length));
  const label = (node: FlowNode) => t(`dataflows.endpointTypes.${node.endpoint.object_type.replaceAll(".", "_")}`);
  return <SurfaceCard role="region" aria-label={t("dataflows.matrixTitle")} className="overflow-hidden">
    <div tabIndex={0} role="region" aria-label={t("dataflows.matrixScroll")} className="max-h-[60dvh] overflow-auto focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"><table className="w-full border-collapse text-xs"><thead><tr><th className="sticky left-0 top-0 z-20 min-w-28 bg-card p-2 text-left">{t("dataflows.matrixAxes")}</th>{graph.nodes.map(node => <th key={node.key} scope="col" className="sticky top-0 min-w-24 border border-border bg-card p-2">{label(node)}</th>)}</tr></thead><tbody>{graph.nodes.map(source => <tr key={source.key}><th scope="row" className="sticky left-0 z-10 border border-border bg-card p-2 text-left">{label(source)}</th>{graph.nodes.map(destination => {
      const cell = cells.get(JSON.stringify([source.key, destination.key]));
      return <td key={destination.key} className="border border-border p-1 text-center">{cell ? <button type="button" aria-pressed={selected === cell.key} aria-label={t("dataflows.edgeBucket", { source: label(source), destination: label(destination), count: cell.flows.length })} onClick={() => setSelected(cell.key)} className="relative h-12 w-full min-w-16 overflow-hidden rounded-sm font-semibold text-primary focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><span aria-hidden="true" className="absolute inset-0 bg-primary" style={{ opacity: 0.08 + 0.3 * Math.sqrt(cell.flows.length / max) }} /><span className="relative">{cell.flows.length} →</span></button> : <span className="text-muted-foreground">—</span>}</td>;
    })}</tr>)}</tbody></table></div>
    {edge ? <DataflowDrilldown key={edge.key} flows={edge.flows} onInspect={onInspect} /> : <p className="p-4 text-xs text-muted-foreground">{t("dataflows.selectBucket")}</p>}
  </SurfaceCard>;
}
