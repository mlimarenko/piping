import { useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ArrowLeft, ChevronRight, LocateFixed, MapPin, MapPinOff, Loader2, Search } from "lucide-react";
import { listAllResources } from "@/features/inventory/api";
import InventoryMapView from "@/features/inventory/components/map/InventoryMapView";
import type { ConnectionTarget } from "@/shared/api/types";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import { groupWorkspaceLocations, projectWorkspaceTargets, type WorkspaceMapLocation } from "./workspace-map-model";
import WorkspaceMapTargets from "./WorkspaceMapTargets";

interface WorkspaceMapProps {
  targets: readonly ConnectionTarget[];
  selectedTarget: ConnectionTarget | null;
  onSelect: (target: ConnectionTarget) => void;
}

// Mounted only behind Workspace's READ INVENTORY gate and lazy boundary.
export default function WorkspaceMap({ targets, selectedTarget, onSelect }: WorkspaceMapProps) {
  const { t } = useTranslation();
  const [fitKey, setFitKey] = useState(0);
  const [unplacedOpen, setUnplacedOpen] = useState(false);
  const [locationId, setLocationId] = useState<string | null>(null);
  const [locationQuery, setLocationQuery] = useState("");
  const [locationPage, setLocationPage] = useState(0);
  const suppressFocusOpen = useRef(false);
  const unplacedTrigger = useRef<HTMLButtonElement>(null);
  const unplacedContent = useRef<HTMLDivElement>(null);
  const locationsTrigger = useRef<HTMLButtonElement>(null);
  const backButton = useRef<HTMLButtonElement>(null);
  const interactedOutside = useRef(false);
  const sites = useQuery({ queryKey: ["inventory", "resources", "Sites", "workspace-map"], queryFn: () => listAllResources("Sites") });
  const { located, unplaced } = useMemo(() => projectWorkspaceTargets(targets, sites.data ?? []), [targets, sites.data]);
  const groups = useMemo(() => groupWorkspaceLocations(located), [located]);
  const locationName = (group: WorkspaceMapLocation) => group.sites.length === 1 ? group.name : t("connections.map.coLocatedSites", { name: group.name, count: group.sites.length - 1 });
  const points = useMemo(() => groups.map(group => ({ id: group.id, label: group.sites.length === 1 ? group.name : t("connections.map.coLocatedSites", { name: group.name, count: group.sites.length - 1 }), lat: group.lat, lng: group.lng, labelCount: group.targets.length, radius: 14 })), [groups, t]);
  const selectedLocation = groups.find(group => group.id === locationId);
  const selectedGroup = groups.find(group => group.targets.some(target => target.id === selectedTarget?.id));
  const visibleGroups = groups.filter(group => group.sites.some(site => site.name.toLowerCase().includes(locationQuery.trim().toLowerCase())));
  const page = Math.min(locationPage, Math.max(0, Math.ceil(visibleGroups.length / 25) - 1));
  const openLocation = (id: string | null) => { setUnplacedOpen(false); setLocationId(id); if (id) requestAnimationFrame(() => backButton.current?.focus()); };
  const back = () => { setLocationId(null); requestAnimationFrame(() => locationsTrigger.current?.focus()); };
  const openUnplacedFromKeyboard = (event: React.KeyboardEvent<HTMLButtonElement>) => {
    if (!["ArrowDown", "Enter", " "].includes(event.key)) return;
    event.preventDefault();
    setUnplacedOpen(true);
    requestAnimationFrame(() => unplacedContent.current?.querySelector("input")?.focus());
  };

  if (sites.isPending) return <div role="status" className="flex flex-1 items-center justify-center gap-2 p-8 text-sm text-muted-foreground"><Loader2 className="size-4 animate-spin" />{t("connections.map.loading")}</div>;
  if (sites.isError) return <div role="alert" className="flex flex-col items-center justify-center gap-4 p-8 text-center text-sm"><p>{t("connections.map.error")}</p><Button variant="outline" onClick={() => void sites.refetch()}>{t("connections.catalogue.retry")}</Button></div>;

  return <section aria-label={t("connections.views.map")} className="relative isolate flex min-h-[440px] flex-1 flex-col overflow-hidden rounded-lg border border-border bg-card">
    <div className="flex flex-wrap items-center justify-between gap-2 border-b border-border bg-card px-3 py-2">
      <Button ref={locationsTrigger} variant="ghost" size="sm" className="h-10 min-w-0 gap-2 px-0" onClick={back}><MapPin className="size-4 shrink-0 text-primary" /><span>{t("connections.map.placed", { count: located.length, sites: new Set(located.map(item => item.site.id)).size })}</span></Button>
      <div className="flex items-center gap-2">
        {unplaced.length > 0 && <Popover open={unplacedOpen} onOpenChange={open => { suppressFocusOpen.current = !open; setUnplacedOpen(open); }}>
          <PopoverTrigger asChild><Button ref={unplacedTrigger} variant="ghost" size="sm" aria-label={t("connections.map.unplacedTitle", { count: unplaced.length })} className="h-10 gap-2 px-2 text-muted-foreground" onKeyDown={openUnplacedFromKeyboard} onPointerEnter={event => { if (event.pointerType === "mouse") setUnplacedOpen(true); }} onFocus={() => { if (!suppressFocusOpen.current) setUnplacedOpen(true); suppressFocusOpen.current = false; }} onClick={event => { event.preventDefault(); setUnplacedOpen(true); }}><MapPinOff className="size-4" /><span className="hidden sm:inline">{t("connections.map.unplaced")}</span><span className="rounded-md bg-muted px-2 py-0.5 text-xs font-medium text-foreground">{unplaced.length}</span></Button></PopoverTrigger>
          <PopoverContent ref={unplacedContent} aria-label={t("connections.map.unplacedTitle", { count: unplaced.length })} align="end" className="z-[1300] flex max-h-[70dvh] w-[min(22rem,calc(100vw-2rem))] flex-col overflow-hidden p-0" onOpenAutoFocus={event => { event.preventDefault(); interactedOutside.current = false; }} onInteractOutside={event => { if (!unplacedTrigger.current?.contains(event.target as Node)) interactedOutside.current = true; }} onCloseAutoFocus={event => { event.preventDefault(); if (!interactedOutside.current) { suppressFocusOpen.current = true; unplacedTrigger.current?.focus(); suppressFocusOpen.current = false; } }}>
            <div className="border-b border-border p-4"><h2 className="text-sm font-semibold">{t("connections.map.unplacedTitle", { count: unplaced.length })}</h2><p className="mt-1 text-xs text-muted-foreground">{t("connections.map.unplacedHint")}</p></div>
            <WorkspaceMapTargets targets={unplaced.map(item => item.target)} reasons={unplaced} selectedTarget={selectedTarget} onSelect={target => { setUnplacedOpen(false); onSelect(target); }} />
          </PopoverContent>
        </Popover>}
        <Button variant="outline" size="icon" className="size-10" disabled={points.length === 0} onClick={() => setFitKey(value => value + 1)} aria-label={t("connections.map.fit")} title={t("connections.map.fit")}><LocateFixed className="size-4" /></Button>
      </div>
    </div>
    <div className="flex min-h-0 flex-1 flex-col md:flex-row">
      <div className={`relative shrink-0 md:order-2 md:h-auto md:min-h-[320px] md:flex-1 ${selectedLocation ? "h-24" : "h-48"}`}>
        <InventoryMapView className="absolute inset-0" points={points} selectedId={selectedLocation?.id ?? selectedGroup?.id} onSelect={openLocation} fitKey={fitKey} />
        {points.length === 0 && <div className="pointer-events-none absolute inset-0 flex items-center justify-center p-4"><div className="max-w-sm rounded-lg border border-border bg-card/95 p-4 text-center shadow-sm"><h2 className="text-sm font-semibold">{t("connections.map.noLocations")}</h2><p className="mt-2 text-xs text-muted-foreground">{t("connections.map.noLocationsHint")}</p></div></div>}
      </div>
      <aside aria-label={t("connections.map.locations")} className="flex min-h-0 flex-1 flex-col border-t border-border bg-card md:order-1 md:w-80 md:flex-none md:border-r md:border-t-0" onKeyDown={event => { if (event.key === "Escape" && selectedLocation) { event.stopPropagation(); back(); } }}>
        {selectedLocation ? <>
          <div className="flex items-center gap-2 border-b border-border p-3 md:flex-col md:items-start"><Button ref={backButton} variant="ghost" size="sm" className="h-10 shrink-0 gap-1 px-2 text-muted-foreground md:h-9 md:px-0" onClick={back}><ArrowLeft className="size-4" /><span className="sr-only md:not-sr-only">{t("connections.map.allLocations")}</span></Button><div className="min-w-0"><h2 className="text-base font-semibold leading-tight">{selectedLocation.name}</h2><p className="mt-1 text-xs text-muted-foreground">{t("connections.map.locationSummary", { count: selectedLocation.targets.length, sites: selectedLocation.sites.length })}</p></div></div>
          <WorkspaceMapTargets key={selectedLocation.id} targets={selectedLocation.targets} sites={selectedLocation.sites} selectedTarget={selectedTarget} onSelect={onSelect} />
        </> : <>
          <div className="flex flex-col gap-2 border-b border-border p-3"><h2 className="text-sm font-semibold">{t("connections.map.locations")}</h2><p className="text-xs text-muted-foreground">{t("connections.map.chooseLocation")}</p><div className="relative"><Search className="pointer-events-none absolute left-3 top-3 size-4 text-muted-foreground" /><Input aria-label={t("connections.map.searchLocations")} placeholder={t("connections.map.searchLocations")} value={locationQuery} onChange={event => { setLocationQuery(event.target.value); setLocationPage(0); }} className="h-10 pl-9" /></div></div>
          <ul className="min-h-0 flex-1 overflow-y-auto p-2">{visibleGroups.slice(page * 25, page * 25 + 25).map(group => <li key={group.id}><button type="button" onClick={() => openLocation(group.id)} className="flex min-h-16 w-full items-center gap-3 rounded-md px-2 py-2 text-left hover:bg-accent focus-visible:outline focus-visible:outline-2 focus-visible:outline-ring"><span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"><MapPin className="size-5" /></span><span className="min-w-0 flex-1"><span className="block truncate text-sm font-medium">{locationName(group)}</span><span className="text-xs text-muted-foreground">{t("connections.map.locationSummary", { count: group.targets.length, sites: group.sites.length })}</span></span><ChevronRight className="size-4 shrink-0 text-muted-foreground" /></button></li>)}{visibleGroups.length === 0 && <li className="p-4 text-sm text-muted-foreground">{t("connections.map.noResults")}</li>}</ul>
          {visibleGroups.length > 25 && <div className="flex items-center justify-between gap-2 border-t border-border p-2"><Button variant="outline" size="sm" disabled={page === 0} onClick={() => setLocationPage(page - 1)}>{t("connections.map.previous")}</Button><span className="text-xs">{page + 1} / {Math.ceil(visibleGroups.length / 25)}</span><Button variant="outline" size="sm" disabled={(page + 1) * 25 >= visibleGroups.length} onClick={() => setLocationPage(page + 1)}>{t("connections.map.next")}</Button></div>}
        </>}
      </aside>
    </div>
  </section>;
}
