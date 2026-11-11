import { useMemo, useRef, useState, type ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ChevronRight, LocateFixed, MapPin, MapPinOff, Loader2 } from "lucide-react";
import { listAllResources } from "@/features/inventory/api";
import InventoryMapView from "@/features/inventory/components/map/InventoryMapView";
import type { ConnectionTarget } from "@/shared/api/types";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { TablePagination } from "@/shared/ui-shared/Pagination";
import { groupWorkspaceLocations, projectWorkspaceTargets, type WorkspaceMapLocation } from "./workspace-map-model";
import WorkspaceMapTargets from "./WorkspaceMapTargets";

interface WorkspaceMapProps {
  targets: readonly ConnectionTarget[];
  selectedTarget: ConnectionTarget | null;
  onSelect: (target: ConnectionTarget) => void;
  renderActions?: (target: ConnectionTarget) => ReactNode;
  onIntent?: (target: ConnectionTarget) => void;
}

export default function WorkspaceMap({ targets, selectedTarget, onSelect, renderActions, onIntent }: WorkspaceMapProps) {
  const { t } = useTranslation();
  const [fitKey, setFitKey] = useState(0);
  const [focus, setFocus] = useState<{ id: string; key: number } | null>(null);
  const [locationId, setLocationId] = useState<string | null>(null);
  const [unplacedOpen, setUnplacedOpen] = useState(false);
  const [mobileView, setMobileView] = useState<"map" | "list">("map");
  const [locationQuery, setLocationQuery] = useState("");
  const [locationPage, setLocationPage] = useState(0);
  const [locationPageSize, setLocationPageSize] = useState(25);
  const locationsTrigger = useRef<HTMLButtonElement>(null);
  const backButton = useRef<HTMLButtonElement>(null);
  const sites = useQuery({ queryKey: ["inventory", "resources", "Sites", "workspace-map"], queryFn: () => listAllResources("Sites") });
  const { located, unplaced } = useMemo(() => projectWorkspaceTargets(targets, sites.data ?? []), [targets, sites.data]);
  const groups = useMemo(() => groupWorkspaceLocations(located), [located]);
  const locationName = (group: WorkspaceMapLocation) => group.sites.length === 1 ? group.name : t("connections.map.coLocatedSites", { name: group.name, count: group.sites.length - 1 });
  const selectedLocation = groups.find(group => group.id === locationId);
  const selectedGroup = groups.find(group => group.targets.some(target => target.id === selectedTarget?.id));
  const visibleGroups = locationId || unplacedOpen ? groups : groups.filter(group => group.sites.some(site => site.name.toLowerCase().includes(locationQuery.trim().toLowerCase())));
  const points = visibleGroups.map(group => ({ id: group.id, label: locationName(group), lat: group.lat, lng: group.lng, labelCount: group.targets.length, radius: 14 }));
  const page = Math.min(locationPage, Math.max(0, Math.ceil(visibleGroups.length / locationPageSize) - 1));
  const openLocation = (id: string | null) => {
    setUnplacedOpen(false);
    setLocationId(id);
    if (id) {
      setFocus(previous => ({ id, key: (previous?.key ?? 0) + 1 }));
      setMobileView("list");
      requestAnimationFrame(() => backButton.current?.focus());
    }
  };
  const back = () => { setLocationId(null); setUnplacedOpen(false); requestAnimationFrame(() => locationsTrigger.current?.focus()); };

  if (sites.isPending) return <div role="status" className="flex flex-1 items-center justify-center gap-2 p-8 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{t("connections.map.loading")}</div>;
  if (sites.isError) return <div role="alert" className="flex flex-col items-center justify-center gap-4 p-8 text-center text-sm"><p>{t("connections.map.error")}</p><Button variant="outline" className="min-h-11" onClick={() => void sites.refetch()}>{t("connections.catalogue.retry")}</Button></div>;

  return <section aria-label={t("connections.views.map")} className="relative isolate flex min-h-[440px] min-w-0 flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card" onKeyDown={event => { if (event.key === "Escape" && (selectedLocation || unplacedOpen)) { event.stopPropagation(); back(); } }}>
    <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border bg-card px-3 py-2">
      <Button ref={locationsTrigger} variant="ghost" size="sm" className="min-h-11 min-w-0 gap-2 px-0" onClick={back}><MapPin className="size-4 shrink-0 text-primary" /><span>{t("connections.map.placed", { count: located.length, sites: new Set(located.map(item => item.site.id)).size })}</span></Button>
      <div className="flex items-center gap-2">
        {unplaced.length > 0 && <Button variant={unplacedOpen ? "secondary" : "ghost"} size="sm" aria-label={t("connections.map.unplacedTitle", { count: unplaced.length })} className="min-h-11 gap-2 px-2" onClick={() => { setLocationId(null); setUnplacedOpen(true); setMobileView("list"); }}><MapPinOff className="size-4" /><span>{t("connections.map.unplaced")}</span><span className="rounded-md bg-muted px-2 py-0.5 text-xs">{unplaced.length}</span></Button>}
        <Button variant="outline" size="icon" className="size-11" disabled={groups.length === 0} onClick={() => { setLocationQuery(""); setLocationPage(0); setFitKey(value => value + 1); }} aria-label={t("connections.map.fit")} title={t("connections.map.fit")}><LocateFixed className="size-4" /></Button>
      </div>
    </div>
    <div className="flex gap-2 border-b border-border p-2 md:hidden">
      <Button variant={mobileView === "map" ? "secondary" : "ghost"} className="min-h-11 flex-1" aria-pressed={mobileView === "map"} onClick={() => setMobileView("map")}>{t("connections.views.map")}</Button>
      <Button variant={mobileView === "list" ? "secondary" : "ghost"} className="min-h-11 flex-1" aria-pressed={mobileView === "list"} onClick={() => setMobileView("list")}>{t("connections.map.list")}</Button>
    </div>
    <div className="relative flex min-h-0 flex-1 flex-col md:flex-row">
      <div className={`min-h-[320px] flex-1 md:relative md:order-2 ${mobileView === "map" ? "relative" : "invisible absolute inset-0 md:visible"}`}>
        <InventoryMapView className="absolute inset-0" points={points} selectedId={selectedLocation?.id ?? selectedGroup?.id} focusId={focus?.id} focusKey={focus?.key} onSelect={openLocation} fitKey={fitKey} />
        {points.length === 0 && !locationQuery && <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-4"><div className="max-w-sm rounded-lg border border-border bg-card/95 p-4 text-center shadow-sm"><h2 className="text-sm font-semibold">{t("connections.map.noLocations")}</h2><p className="mt-2 text-xs text-muted-foreground">{t("connections.map.noLocationsHint")}</p></div></div>}
      </div>
      <aside aria-label={t("connections.map.locations")} className={`min-h-0 flex-1 flex-col bg-card md:order-1 md:flex md:w-[22rem] md:flex-none md:border-r md:border-border ${mobileView === "list" ? "flex" : "hidden"}`}>
        {selectedLocation || unplacedOpen ? <>
          <div className="flex items-center gap-2 border-b border-border px-3 py-2"><Button ref={backButton} variant="ghost" size="icon" className="size-11 shrink-0" aria-label={t("connections.map.allLocations")} title={t("connections.map.allLocations")} onClick={back}><ArrowLeft className="size-4" /></Button><div className="min-w-0"><h2 className="line-clamp-2 break-words text-base font-semibold leading-tight" title={selectedLocation?.name}>{selectedLocation?.name ?? t("connections.map.unplacedTitle", { count: unplaced.length })}</h2>{selectedLocation && selectedLocation.sites.length > 1 && <p className="text-xs text-muted-foreground">{t("connections.map.locationSummary", { count: selectedLocation.targets.length, sites: selectedLocation.sites.length })}</p>}</div></div>
          <WorkspaceMapTargets key={selectedLocation?.id ?? "unplaced"} targets={selectedLocation?.targets ?? unplaced.map(item => item.target)} sites={selectedLocation?.sites} reasons={unplacedOpen ? unplaced : undefined} selectedTarget={selectedTarget} onSelect={onSelect} renderActions={renderActions} onIntent={onIntent} />
        </> : <>
          <div className="flex flex-col gap-2 border-b border-border p-3"><h2 className="text-sm font-semibold">{t("connections.map.locations")}</h2><p className="text-xs text-muted-foreground">{t("connections.map.chooseLocation")}</p><SearchInput aria-label={t("connections.map.searchLocations")} placeholder={t("connections.map.searchLocations")} value={locationQuery} onValueChange={value => { setLocationQuery(value); setLocationPage(0); }} onClear={() => { setLocationQuery(""); setLocationPage(0); }} clearLabel={t("connections.map.clearSearch")} containerClassName="sm:w-full" className="h-11" /></div>
          <ul className="min-h-0 flex-1 overflow-y-auto p-2">{visibleGroups.slice(page * locationPageSize, (page + 1) * locationPageSize).map(group => <li key={group.id}><button type="button" onClick={() => openLocation(group.id)} className="flex min-h-16 w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"><span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"><MapPin className="size-5" /></span><span className="min-w-0 flex-1"><span title={locationName(group)} className="line-clamp-2 break-words text-sm font-medium">{locationName(group)}</span><span className="text-xs text-muted-foreground">{t("connections.map.locationSummary", { count: group.targets.length, sites: group.sites.length })}</span></span><ChevronRight className="size-4 shrink-0 text-muted-foreground" /></button></li>)}{visibleGroups.length === 0 && <li className="p-4 text-sm text-muted-foreground">{t("connections.map.noResults")}</li>}</ul>
          {visibleGroups.length > 25 && <nav aria-label={t("connections.map.pages")} className="border-t border-border p-2"><TablePagination compact page={page} pageSize={locationPageSize} total={visibleGroups.length} pageSizeOptions={[25, 50, 100]} onPageChange={setLocationPage} onPageSizeChange={size => { setLocationPageSize(size); setLocationPage(0); }} /></nav>}
        </>}
      </aside>
    </div>
  </section>;
}
