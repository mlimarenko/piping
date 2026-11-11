import { useMemo, useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Monitor, Search, Server } from "lucide-react";
import type { ConnectionTarget, Resource } from "@/shared/api/types";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { TablePagination } from "@/shared/ui-shared/Pagination";
import { matchesTargetSearch } from "./components/connections/filters-utils";
import type { UnplacedWorkspaceTarget } from "./workspace-map-model";

interface Props {
  targets: readonly ConnectionTarget[];
  selectedTarget: ConnectionTarget | null;
  onSelect: (target: ConnectionTarget) => void;
  reasons?: readonly UnplacedWorkspaceTarget[];
  sites?: readonly Resource[];
  renderActions?: (target: ConnectionTarget) => ReactNode;
  onIntent?: (target: ConnectionTarget) => void;
}

export default function WorkspaceMapTargets({ targets, selectedTarget, onSelect, reasons, sites, renderActions, onIntent }: Props) {
  const { t } = useTranslation();
  const searchLabel = reasons ? t("connections.map.searchUnplacedList", { defaultValue: "Search unplaced targets" }) : t("connections.map.searchLocationList", { defaultValue: "Search targets in this location" });
  const [query, setQuery] = useState("");
  const [kind, setKind] = useState("all");
  const [siteId, setSiteId] = useState("");
  const [page, setPage] = useState(0);
  const [pageSize, setPageSize] = useState(25);
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
  const currentPage = Math.min(page, Math.max(0, Math.ceil(filtered.length / pageSize) - 1));
  const start = currentPage * pageSize;
  const clear = () => { setQuery(""); setKind("all"); setSiteId(""); setPage(0); input.current?.focus(); };
  const changePage = (next: number) => { setPage(next); list.current?.scrollTo?.(0, 0); };

  return <div className="flex min-h-0 flex-1 flex-col">
    <div className="flex flex-col gap-2 border-b border-border p-3">
      <SearchInput ref={input} aria-label={searchLabel} placeholder={searchLabel} value={query} onValueChange={value => { setQuery(value); setPage(0); }} onClear={clear} clearLabel={t("connections.map.clearSearch")} containerClassName="sm:w-full" className="h-11" />
      {!query && (kind !== "all" || siteId) && <Button variant="ghost" className="min-h-11" onClick={clear}>{t("connections.map.clearSearch")}</Button>}
      {sites && sites.length > 1 && <select aria-label={t("connections.map.siteFilter")} value={siteId} onChange={event => { setSiteId(event.target.value); setPage(0); }} className="control-surface h-11 w-full min-w-0 rounded-md px-2 text-sm"><option value="">{t("connections.map.allSites", { count: sites.length })}</option>{sites.map(site => <option key={site.id} value={site.id}>{site.name} ({siteCounts.get(site.id) ?? 0})</option>)}</select>}
      <div className="flex flex-wrap gap-1" aria-label={t("connections.map.targetTypes")}>
        {[["all", scoped.length], ["device", devices], ["virtual_machine", scoped.length - devices]].map(([value, count]) => <Button key={value} variant={kind === value ? "secondary" : "ghost"} size="sm" className="min-h-11 gap-1 px-2 text-xs" aria-pressed={kind === value} onClick={() => { setKind(String(value)); setPage(0); }}>{t(`connections.map.kinds.${value}`)} <span className="tabular-nums text-muted-foreground">{count}</span></Button>)}
      </div>
      {(query || kind !== "all" || siteId) && <p role="status" className="text-xs text-muted-foreground">{t("connections.map.results", { count: filtered.length, total: targets.length })}</p>}
    </div>
    <ul ref={list} aria-label={t("connections.map.targetResults")} className="min-h-0 flex-1 overflow-y-auto p-2">
      {filtered.slice(start, start + pageSize).map(target => {
        const reason = reasonById.get(target.id);
        const Icon = target.targetKind === "device" ? Server : Monitor;
        return <li key={target.id} className="flex min-w-0 items-center" onPointerEnter={() => onIntent?.(target)} onFocusCapture={() => onIntent?.(target)}><button type="button" aria-pressed={selectedTarget?.id === target.id} onClick={() => onSelect(target)} className="flex min-h-14 min-w-0 flex-1 items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring aria-pressed:bg-secondary"><Icon className="size-4 shrink-0 text-muted-foreground" /><span className="flex min-w-0 flex-1 flex-col gap-1"><span title={target.name} className="line-clamp-2 break-words text-sm font-medium">{target.name}</span><span className="line-clamp-2 break-words text-xs text-muted-foreground">{reason ? t(`connections.map.reasons.${reason}`) : [target.primaryIp, sites?.length === 1 || siteId ? null : target.site].filter(Boolean).join(" · ") || t(`connections.map.kinds.${target.targetKind}`)}</span></span></button>{renderActions?.(target)}</li>;
      })}
      {filtered.length === 0 && <li className="flex flex-col items-center gap-2 p-6 text-center"><Search className="size-6 text-muted-foreground" /><p className="text-sm">{t("connections.map.noResults")}</p><Button variant="outline" size="sm" className="min-h-11" onClick={clear}>{t("connections.map.clearSearch")}</Button></li>}
    </ul>
    {filtered.length > 25 && <nav aria-label={t("connections.map.pages")} className="shrink-0 border-t border-border p-2"><TablePagination compact page={currentPage} pageSize={pageSize} total={filtered.length} pageSizeOptions={[25, 50, 100]} onPageChange={changePage} onPageSizeChange={size => { setPageSize(size); changePage(0); }} /></nav>}
  </div>;
}
