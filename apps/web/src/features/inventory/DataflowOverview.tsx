import { useMemo, useState, type ReactNode, type RefObject } from "react";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import { useTranslation } from "react-i18next";
import { ArrowRight } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import PageState from "@/shared/ui-shared/PageState";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import type { Dataflow } from "./dataflows-api";
import { EndpointLabel, FlowStatus, FlowTraffic } from "./DataflowConnection";
import DataflowDetail from "./DataflowDetail";
import DataflowDrilldown from "./DataflowDrilldown";
import DataflowTopology from "./DataflowTopology";
import DataflowMatrix from "./DataflowMatrix";
import { buildDataflowOverview, buildDataflowVisualization, type DataflowView, type OverviewObject } from "./dataflowVisualization";
import { dataflowEndpointKey } from "./dataflowEndpoints";

const RELATIONSHIP_LIMIT = 8;
const OBJECT_LIMIT = 6;
const OBJECT_SEARCH_LIMIT = 20;

export default function DataflowOverview({ flows, loading, failed, onRetry, renderActions, view, selected, onSelect, anchor, writeFailed = false }: Readonly<{
  flows: Dataflow[]; loading: boolean; failed: boolean; onRetry: () => void;
  renderActions?: (flow: Dataflow) => ReactNode;
  view: DataflowView; selected?: Dataflow; onSelect: (id: string | null) => void;
  writeFailed?: boolean;
  anchor: RefObject<HTMLElement | null>;
}>) {
  const { t } = useTranslation();
  const focus = useCollectionPreviewFocus(anchor);
  const inspect = (flow: Dataflow, button: HTMLButtonElement) => { focus.capture(button); onSelect(flow.id); };
  const visualization = useMemo(() => buildDataflowVisualization(flows), [flows]);
  const overviewModel = useMemo(() => buildDataflowOverview(flows), [flows]);
  const [relationshipsExpanded, setRelationshipsExpanded] = useState(false);
  const [openRelationship, setOpenRelationship] = useState<string | null>(null);
  const [objectSearch, setObjectSearch] = useState("");
  const [objectsExpanded, setObjectsExpanded] = useState(false);
  const [openObject, setOpenObject] = useState<string | null>(null);

  const typeLabel = (type: Dataflow["sources"][number]["object_type"]) => t(`dataflows.endpointTypes.${type.replaceAll(".", "_")}`);
  const objectName = (object: OverviewObject) => object.node.endpoint.label ?? typeLabel(object.node.endpoint.object_type);

  const relationships = overviewModel.relationships;
  const visibleRelationships = relationshipsExpanded ? relationships : relationships.slice(0, RELATIONSHIP_LIMIT);
  const openRelationshipItem = relationships.find(item => item.key === openRelationship);

  const search = objectSearch.trim().toLocaleLowerCase();
  const matchingObjects = useMemo(() => search
    ? overviewModel.objects.filter(item => [item.node.endpoint.label, item.node.endpoint.object_id].filter(Boolean).join(" ").toLocaleLowerCase().includes(search))
    : overviewModel.objects, [overviewModel.objects, search]);
  const objectLimit = search ? OBJECT_SEARCH_LIMIT : OBJECT_LIMIT;
  const visibleObjects = objectsExpanded ? matchingObjects : matchingObjects.slice(0, objectLimit);
  const openObjectItem = overviewModel.objects.find(item => item.node.key === openObject);
  const openObjectFlows = useMemo(() => openObject
    ? visualization.exact.edges.filter(edge => edge.source === openObject || edge.destination === openObject).flatMap(edge => edge.flows)
    : [], [visualization, openObject]);

  const toggleRelationship = (key: string) => { setOpenObject(null); setOpenRelationship(current => current === key ? null : key); };
  const toggleObject = (key: string) => { setOpenRelationship(null); setOpenObject(current => current === key ? null : key); };

  return <div className="flex min-w-0 flex-col gap-4">
    {loading ? <SurfaceCard><PageState kind="loading" title={t("dataflows.overviewLoading")} /></SurfaceCard> : failed ? <SurfaceCard><PageState kind="error" title={t("dataflows.loadFailed")} action={<Button variant="outline" onClick={onRetry}>{t("retry")}</Button>} /></SurfaceCard> : <>
    {view === "topology" && flows.length > 0 && <DataflowTopology flows={flows} onInspect={inspect} />}
    {view === "overview" && flows.length > 0 && <SurfaceCard role="region" aria-label={t("dataflows.groupedTitle")} className="grid min-w-0 lg:grid-cols-2">
      <section className="flex min-w-0 flex-col gap-2 border-b border-border p-4 lg:border-b-0 lg:border-r">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="text-sm font-semibold">{t("dataflows.overviewRelationships")}</h2>
          <output className="text-xs text-muted-foreground">{t("dataflows.overviewRelationshipsShown", { shown: visibleRelationships.length, total: overviewModel.totalRelationships })}</output>
        </div>
        <ul className={relationshipsExpanded ? "max-h-96 divide-y divide-border overflow-y-auto" : "divide-y divide-border"}>
          {visibleRelationships.map(relationship => <li key={relationship.key}>
            <button type="button" aria-expanded={openRelationship === relationship.key} aria-label={t("dataflows.edgeBucket", { source: typeLabel(relationship.sourceType), destination: typeLabel(relationship.destinationType), count: relationship.flows.length })} onClick={() => toggleRelationship(relationship.key)} className="flex w-full min-w-0 flex-wrap items-center justify-between gap-2 rounded-sm px-2 py-2 text-left hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
              <span className="flex min-w-0 items-center gap-2">
                <span className="truncate text-sm">{typeLabel(relationship.sourceType)}</span>
                <ArrowRight aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
                <span className="truncate text-sm">{typeLabel(relationship.destinationType)}</span>
              </span>
              <span className="text-xs tabular-nums text-muted-foreground">{t("dataflows.flowCount", { count: relationship.flows.length })}</span>
            </button>
          </li>)}
        </ul>
        {relationships.length > RELATIONSHIP_LIMIT && <Button size="sm" variant="ghost" className="self-start" onClick={() => setRelationshipsExpanded(expanded => !expanded)}>{relationshipsExpanded ? t("dataflows.showLess") : t("dataflows.showAll")}</Button>}
        {openRelationshipItem && <DataflowDrilldown key={openRelationshipItem.key} flows={openRelationshipItem.flows} onInspect={inspect} />}
      </section>
      <section className="flex min-w-0 flex-col gap-2 p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h2 className="text-sm font-semibold">{t("dataflows.overviewObjects")}</h2>
          <output className="text-xs text-muted-foreground">{t("dataflows.overviewObjectsShown", { shown: visibleObjects.length, total: overviewModel.totalObjects })}</output>
        </div>
        <Input aria-label={t("dataflows.searchObjects")} placeholder={t("dataflows.searchObjects")} value={objectSearch} onChange={event => { setObjectSearch(event.target.value); setObjectsExpanded(false); }} />
        {matchingObjects.length ? <>
          <ul className={objectsExpanded || matchingObjects.length > objectLimit ? "max-h-96 divide-y divide-border overflow-y-auto" : "divide-y divide-border"}>
            {visibleObjects.map(object => <li key={object.node.key}>
              <button type="button" aria-expanded={openObject === object.node.key} aria-label={t("dataflows.inspectObject", { name: objectName(object) })} onClick={() => toggleObject(object.node.key)} className="flex w-full min-w-0 flex-col gap-1 rounded-sm px-2 py-2 text-left hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
                <span className="min-w-0 break-words text-sm font-medium [overflow-wrap:anywhere]">{objectName(object)}</span>
                <span className="flex flex-wrap items-center gap-2 text-xs tabular-nums text-muted-foreground">
                  <span>{typeLabel(object.node.endpoint.object_type)}</span>
                  <span>{t("dataflows.objectFlowCount", { count: object.total })}</span>
                  <span>{t("dataflows.objectIncomingCount", { count: object.incoming })}</span>
                  <span>{t("dataflows.objectOutgoingCount", { count: object.outgoing })}</span>
                </span>
              </button>
            </li>)}
          </ul>
          {matchingObjects.length > objectLimit && <Button size="sm" variant="ghost" className="self-start" onClick={() => setObjectsExpanded(expanded => !expanded)}>{objectsExpanded ? t("dataflows.showLess") : t("dataflows.showAll")}</Button>}
          {openObjectItem && openObjectFlows.length > 0 && <DataflowDrilldown key={openObjectItem.node.key} flows={openObjectFlows} onInspect={inspect} />}
        </> : <p className="text-xs text-muted-foreground">{t("dataflows.noOverviewObjects")}</p>}
      </section>
    </SurfaceCard>}
    {view === "matrix" && flows.length > 0 && <DataflowMatrix graph={visualization.matrix} onInspect={inspect} />}
    {!flows.length && <SurfaceCard><PageState kind="empty" title={t("dataflows.empty")} /></SurfaceCard>}
    {view === "list" && flows.length > 0 && <SurfaceCard><ul aria-label={t("dataflows.overviewTitle")} className="divide-y divide-border">
      {flows.map(flow => <li key={flow.id} className="flex min-w-0 items-center gap-2 px-4 transition-colors hover:bg-muted/40">
        <button type="button" aria-haspopup="dialog" aria-label={t("dataflows.inspectFlow", { name: flow.name })} onClick={event => inspect(flow, event.currentTarget)} className="grid min-w-0 flex-1 grid-cols-[minmax(0,1fr)_auto] gap-2 rounded-sm py-4 text-left focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring md:grid-cols-[minmax(0,1fr)_minmax(9rem,0.5fr)_auto] md:items-center md:gap-6">
          <span className="col-span-2 flex min-w-0 flex-col gap-1 md:col-span-1">
            <span className="flex min-w-0 flex-wrap items-center gap-2">
              <span className="flex min-w-0 flex-col gap-1">{flow.sources.map(endpoint => <EndpointLabel key={dataflowEndpointKey(endpoint)} endpoint={endpoint} side={t("dataflows.source")} />)}</span>
              <ArrowRight aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
              <span className="flex min-w-0 flex-col gap-1">{flow.destinations.map(endpoint => <EndpointLabel key={dataflowEndpointKey(endpoint)} endpoint={endpoint} side={t("dataflows.destination")} />)}</span>
            </span>
            <span className="break-words text-xs text-muted-foreground">{flow.name}</span>
          </span>
          <FlowTraffic flow={flow} /><FlowStatus flow={flow} />
        </button>
        {renderActions?.(flow)}
      </li>)}
    </ul></SurfaceCard>}
    </>}
    <DataflowDetail flow={selected} renderActions={renderActions} writeFailed={writeFailed} onClose={() => onSelect(null)} onCloseAutoFocus={focus.onCloseAutoFocus} />
  </div>;
}
