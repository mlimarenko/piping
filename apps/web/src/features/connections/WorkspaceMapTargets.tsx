import { useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronLeft, ChevronRight, Monitor, Search, Server, X } from "lucide-react";
import type { ConnectionTarget, Resource } from "@/shared/api/types";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { matchesTargetSearch } from "./components/connections/filters-utils";
import type { UnplacedWorkspaceTarget } from "./workspace-map-model";

const PAGE_SIZE = 25;

interface Props {
  targets: readonly ConnectionTarget[];
  selectedTarget: ConnectionTarget | null;
  onSelect: (target: ConnectionTarget) => void;
  reasons?: readonly UnplacedWorkspaceTarget[];
  sites?: readonly Resource[];
}

export default function WorkspaceMapTargets({ targets, selectedTarget, onSelect, reasons, sites }: Props) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState("all");
  const [siteId, setSiteId] = useState("");
  const [page, setPage] = useState(0);
  const list = useRef<HTMLUListElement>(null);
  const input = useRef<HTMLInputElement>(null);
  const scoped = useMemo(() => siteId ? targets.filter(target => target.siteId === siteId) : targets, [targets, siteId]);
  const devices = scoped.filter(target => target.targetKind === "device").length;
  const filtered = useMemo(() => scoped.filter(target => matchesTargetSearch(target, query) && (kind === "all" || target.targetKind === kind)), [scoped, query, kind]);
  const siteCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const target of targets) if (target.siteId) counts.set(target.siteId, (counts.get(target.siteId) ?? 0) + 1);
    return counts;
  }, [targets]);
  const reasonById = useMemo(() => new Map(reasons?.map(item => [item.target.id, item.reason])), [reasons]);
  const currentPage = Math.min(page, Math.max(0, Math.ceil(filtered.length / PAGE_SIZE) - 1));
  const start = currentPage * PAGE_SIZE;
  const clear = () => { setQuery(""); setKind("all"); setSiteId(""); setPage(0); input.current?.focus(); };
  const changePage = (next: number) => { setPage(next); list.current?.scrollTo?.(0, 0); };

  return <div className="flex min-h-0 flex-1 flex-col">
    <div className="flex flex-col gap-2 border-b border-border p-3">
      <div className="relative"><Search className="pointer-events-none absolute left-3 top-3 size-4 text-muted-foreground" /><Input ref={input} aria-label={t("connections.map.searchTargets")} placeholder={t("connections.map.searchTargets")} value={query} onChange={event => { setQuery(event.target.value); setPage(0); }} className="h-10 pl-9 pr-10" /><Button variant="ghost" size="icon" className="absolute right-0 top-0 size-10" disabled={!query && kind === "all" && !siteId} onClick={clear} aria-label={t("connections.map.clearSearch")}><X className="size-4" /></Button></div>
      {sites && sites.length > 1 && <select aria-label={t("connections.map.siteFilter")} value={siteId} onChange={event => { setSiteId(event.target.value); setPage(0); }} className="h-10 w-full min-w-0 rounded-md border border-input bg-background px-2 text-sm focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"><option value="">{t("connections.map.allSites", { count: sites.length })}</option>{sites.map(site => <option key={site.id} value={site.id}>{site.name} ({siteCounts.get(site.id) ?? 0})</option>)}</select>}
      <div className="flex flex-wrap gap-1" aria-label={t("connections.map.targetTypes")}>
        {[["all", scoped.length], ["device", devices], ["virtual_machine", scoped.length - devices]].map(([value, count]) => <Button key={value} variant={kind === value ? "secondary" : "ghost"} size="sm" className="min-h-9 gap-1 px-2 text-xs" aria-pressed={kind === value} onClick={() => { setKind(String(value)); setPage(0); }}>{t(`connections.map.kinds.${value}`)} <span className="tabular-nums text-muted-foreground">{count}</span></Button>)}
      </div>
      <p role="status" className="text-xs text-muted-foreground">{t("connections.map.results", { count: filtered.length, total: targets.length })}</p>
    </div>
    <ul ref={list} aria-label={t("connections.map.targetResults")} className="min-h-0 flex-1 overflow-y-auto p-2">
      {filtered.slice(start, start + PAGE_SIZE).map(target => {
        const reason = reasonById.get(target.id);
        const Icon = target.targetKind === "device" ? Server : Monitor;
        return <li key={target.id}><button type="button" aria-pressed={selectedTarget?.id === target.id} onClick={() => onSelect(target)} className="flex min-h-14 w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring aria-pressed:bg-secondary"><Icon className="size-4 shrink-0 text-muted-foreground" /><span className="flex min-w-0 flex-1 flex-col gap-1"><span className="truncate text-sm font-medium">{target.name}</span><span className="truncate text-xs text-muted-foreground">{reason ? t(`connections.map.reasons.${reason}`) : [target.primaryIp, target.site].filter(Boolean).join(" · ") || t(`connections.map.kinds.${target.targetKind}`)}</span></span></button></li>;
      })}
      {filtered.length === 0 && <li className="flex flex-col items-center gap-2 p-6 text-center"><Search className="size-6 text-muted-foreground" /><p className="text-sm">{t("connections.map.noResults")}</p><Button variant="outline" size="sm" onClick={clear}>{t("connections.map.clearSearch")}</Button></li>}
    </ul>
    {filtered.length > PAGE_SIZE && <nav aria-label={t("connections.map.pages")} className="flex shrink-0 items-center justify-between gap-2 border-t border-border p-2"><Button variant="outline" size="icon" className="size-10" disabled={currentPage === 0} onClick={() => changePage(currentPage - 1)} aria-label={t("connections.map.previous")}><ChevronLeft className="size-4" /></Button><span className="text-xs tabular-nums text-muted-foreground">{start + 1}–{Math.min(start + PAGE_SIZE, filtered.length)} / {filtered.length}</span><Button variant="outline" size="icon" className="size-10" disabled={start + PAGE_SIZE >= filtered.length} onClick={() => changePage(currentPage + 1)} aria-label={t("connections.map.next")}><ChevronRight className="size-4" /></Button></nav>}
  </div>;
}
