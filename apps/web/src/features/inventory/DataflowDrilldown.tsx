import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import type { Dataflow } from "./dataflows-api";
import { FlowStatus, FlowTraffic } from "./DataflowConnection";

export default function DataflowDrilldown({ flows, onInspect }: Readonly<{ flows: Dataflow[]; onInspect: (flow: Dataflow, button: HTMLButtonElement) => void }>) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [page, setPage] = useState(0);
  const matches = flows.filter(flow => [flow.id, flow.name, flow.source.label, flow.source.object_id, flow.destination.label, flow.destination.object_id, flow.transport, flow.status, t(`dataflows.statuses.${flow.status}`), flow.destination_ports?.join(" ")].join(" ").toLocaleLowerCase().includes(search.toLocaleLowerCase()));
  const lastPage = Math.max(0, Math.ceil(matches.length / 20) - 1);
  const current = Math.min(page, lastPage);
  return <section aria-label={t("dataflows.bucketTitle")} className="flex min-w-0 flex-col gap-2 border-t border-border p-4">
    <div className="flex flex-wrap items-center justify-between gap-2"><h3 className="text-sm font-medium">{t("dataflows.bucketTitle")}</h3><output className="text-xs text-muted-foreground">{t("dataflows.bucketCount", { count: matches.length, total: flows.length })}</output></div>
    <Input aria-label={t("dataflows.bucketSearch")} placeholder={t("dataflows.bucketSearch")} value={search} onChange={event => { setSearch(event.target.value); setPage(0); }} />
    <ul className="divide-y divide-border">{matches.slice(current * 20, (current + 1) * 20).map(flow => <li key={flow.id}>
      <button type="button" aria-haspopup="dialog" aria-label={t("dataflows.inspectFlow", { name: flow.name })} onClick={event => onInspect(flow, event.currentTarget)} className="flex w-full min-w-0 flex-col gap-1 rounded-sm py-3 text-left hover:bg-muted focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        <span className="break-words text-sm font-medium">{flow.name}</span><span className="break-all text-xs text-muted-foreground">{flow.source.label ?? flow.source.object_type} → {flow.destination.label ?? flow.destination.object_type}</span>
        <span className="break-all text-xs text-muted-foreground">{flow.source.object_type} · {flow.source.object_id ?? "—"} → {flow.destination.object_type} · {flow.destination.object_id ?? "—"}</span>
        <span className="flex flex-wrap items-center gap-2"><FlowTraffic flow={flow} compact /><FlowStatus flow={flow} /></span>
      </button>
    </li>)}</ul>
    <div className="flex items-center justify-between gap-2"><Button size="sm" variant="outline" disabled={current === 0} onClick={() => setPage(current - 1)}>{t("dataflows.previousBucket")}</Button><span className="text-xs tabular-nums">{current + 1} / {lastPage + 1}</span><Button size="sm" variant="outline" disabled={current === lastPage} onClick={() => setPage(current + 1)}>{t("dataflows.nextBucket")}</Button></div>
  </section>;
}
