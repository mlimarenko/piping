import type { ReactNode, RefObject } from "react";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import { useTranslation } from "react-i18next";
import { ArrowRight, Waypoints } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import EmptyState from "@/shared/ui-shared/EmptyState";
import type { Dataflow } from "./dataflows-api";
import { EndpointLabel, FlowStatus, FlowTraffic } from "./DataflowConnection";
import DataflowDetail from "./DataflowDetail";
import DataflowTopology from "./DataflowTopology";

export default function DataflowOverview({ flows, loading, failed, onRetry, renderActions, view, selected, onSelect, anchor, writeFailed = false }: Readonly<{
  flows: Dataflow[]; loading: boolean; failed: boolean; onRetry: () => void;
  renderActions?: (flow: Dataflow) => ReactNode;
  view: "list" | "topology"; selected?: Dataflow; onSelect: (id: string | null) => void;
  writeFailed?: boolean;
  anchor: RefObject<HTMLElement | null>;
}>) {
  const { t } = useTranslation();
  const focus = useCollectionPreviewFocus(anchor);
  const inspect = (flow: Dataflow, button: HTMLButtonElement) => { focus.capture(button); onSelect(flow.id); };
  const largeGraph = flows.length > 80;
  return <div className="flex min-w-0 flex-col gap-4">
    {loading ? <output className="py-8 text-center text-sm text-muted-foreground">{t("dataflows.overviewLoading")}</output> : failed ? <div className="flex items-center justify-center gap-2 py-8"><p role="alert" className="text-sm text-destructive">{t("dataflows.loadFailed")}</p><Button variant="outline" onClick={onRetry}>{t("retry")}</Button></div> : <>
    {view === "topology" && (largeGraph
      ? <output className="rounded-lg border border-border bg-muted/40 p-4 text-sm text-muted-foreground">{t("dataflows.largeTopology")}</output>
      : flows.length > 0 && <DataflowTopology flows={flows} onInspect={inspect} />)}
    {!flows.length && <EmptyState icon={Waypoints} title={t("dataflows.empty")} className="py-16" />}
    {(view === "list" || largeGraph) && flows.length > 0 && <ul aria-label={t("dataflows.overviewTitle")} className="divide-y divide-border rounded-lg border border-border bg-card">
      {flows.map(flow => <li key={flow.id} className="flex min-w-0 items-center gap-2 px-4 transition-colors hover:bg-muted/40">
        <button type="button" aria-haspopup="dialog" aria-label={t("dataflows.inspectFlow", { name: flow.name })} onClick={event => inspect(flow, event.currentTarget)} className="grid min-w-0 flex-1 grid-cols-[minmax(0,1fr)_auto] gap-2 rounded-sm py-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:grid-cols-[minmax(0,1fr)_minmax(9rem,0.5fr)_auto] md:items-center md:gap-6">
          <span className="col-span-2 flex min-w-0 flex-col gap-1 md:col-span-1">
            <span className="grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-2 md:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)]">
              <span className="col-span-2 min-w-0 md:col-span-1"><EndpointLabel endpoint={flow.source} side={t("dataflows.source")} /></span>
              <ArrowRight aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
              <EndpointLabel endpoint={flow.destination} side={t("dataflows.destination")} />
            </span>
            <span title={flow.name} className="truncate text-xs text-muted-foreground">{flow.name}</span>
          </span>
          <FlowTraffic flow={flow} /><FlowStatus flow={flow} />
        </button>
        {renderActions?.(flow)}
      </li>)}
    </ul>}
    </>}
    <DataflowDetail flow={selected} renderActions={renderActions} writeFailed={writeFailed} onClose={() => onSelect(null)} onCloseAutoFocus={focus.onCloseAutoFocus} />
  </div>;
}
