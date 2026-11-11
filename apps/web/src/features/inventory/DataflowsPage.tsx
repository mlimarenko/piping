import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Link, useLocation, useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, Grid2X2, List, Network, Plus, SlidersHorizontal, Waypoints } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import { Popover, PopoverContent, PopoverTrigger } from "@/shared/ui-primitives/popover";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/shared/ui-primitives/dialog";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from "@/shared/ui-primitives/sheet";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SectionLinks from "@/shared/ui-shared/SectionLinks";
import SurfaceCard from "@/shared/ui-shared/SurfaceCard";
import PageState from "@/shared/ui-shared/PageState";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { EndpointSummary, FlowStatus, FlowStatusSummary, FlowTraffic, endpointDisplayName, transportLabel } from "./DataflowConnection";
import { DATAFLOW_ENDPOINTS } from "./dataflowEndpoints";
import { bulkDataflows, dataflowEndpoints, deleteDataflow, listDataflows, updateDataflow } from "./dataflows-api";
import type { Dataflow, DataflowBulkAction, DataflowTransport } from "./dataflows-api";
import { dataflowGroupDescendants, listApplications, listDataflowGroups } from "./dataflowCatalog-api";
import { inventoryResourceOptionsQuery } from "./resourceOptions";
import DataflowPolicyPanel from "./DataflowPolicyPanel";
import DataflowDetail from "./DataflowDetail";
import DataflowOverview from "./DataflowOverview";
import DataflowCatalogNav from "./DataflowCatalogNav";
import type { DataflowView } from "./dataflowVisualization";

const VIEWS = ["list", "overview", "topology", "matrix"] as const;
const STATUSES = ["active", "draft", "disabled"] as const;
const TRANSPORTS: DataflowTransport[] = ["any", "tcp", "udp", "tcp_udp", "sctp", "icmp", "icmpv6"];
const CLEAR_VALUE = "__none__";

function FilterSelect({ label, value, onChange, children }: Readonly<{
  label: string; value: string; onChange: (value: string) => void; children: ReactNode;
}>) {
  return <label className="flex min-w-0 shrink-0 items-center gap-2 text-xs text-muted-foreground">
    <span className="sr-only">{label}</span>
    <select aria-label={label} value={value} onChange={event => onChange(event.target.value)} className="control-surface h-9 max-w-48 rounded-md px-2 text-sm">{children}</select>
  </label>;
}

export default function DataflowsPage() {
  const { t } = useTranslation();
  const searchInput = useRef<HTMLInputElement>(null);
  const { can } = usePermissions();
  const qc = useQueryClient();
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const previewFocus = useCollectionPreviewFocus(searchInput);
  const [params] = useSearchParams();
  const location = useLocation();
  const navigate = useNavigate();
  const search = params.get("q") ?? "";
  const requestedView = params.get("view");
  const invalidView = requestedView !== null && !VIEWS.includes(requestedView as (typeof VIEWS)[number]);
  const view: DataflowView = !invalidView && requestedView ? requestedView as DataflowView : "list";
  const status = params.get("status") ?? "all";
  const invalidStatus = !["all", ...STATUSES].includes(status);
  const application = params.get("application") ?? "";
  const group = params.get("group") ?? "";
  const transport = params.get("transport") ?? "";
  const tag = params.get("tag") ?? "";
  const invalidTransport = !!transport && !TRANSPORTS.includes(transport as DataflowTransport);
  const endpointType = params.get("endpointType");
  const endpointId = params.get("endpointId");
  const endpointSide = params.get("endpointSide") ?? "any";
  const endpointTagObjectType = params.get("endpointTagObjectType") ?? "";
  const validEndpointSide = ["any", "source", "destination"].includes(endpointSide);
  const validEndpointTagScope = !endpointTagObjectType || ["dcim.device", "virtualization.virtualmachine"].includes(endpointTagObjectType);
  const hasEndpointFilter = endpointType !== null || endpointId !== null;
  const validEndpointFilter = !!endpointType && Object.hasOwn(DATAFLOW_ENDPOINTS, endpointType) && !!endpointId && validEndpointSide && validEndpointTagScope && (!endpointTagObjectType || endpointType === "extras.tag");
  const updatedParams = (changes: Record<string, string | null>) => {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(changes)) { if (value) next.set(key, value); else next.delete(key); }
    return next;
  };
  const changeParams = (changes: Record<string, string | null>, replace = false) => {
    void navigate({ pathname: location.pathname, search: `?${updatedParams(changes)}`, hash: location.hash }, { replace });
  };
  const [target, setTarget] = useState<Dataflow | null>(null);
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(() => new Set());
  const [moveOpen, setMoveOpen] = useState(false);
  const [deleteOpen, setDeleteOpen] = useState(false);
  const [moveGroup, setMoveGroup] = useState("");
  const [moveApplication, setMoveApplication] = useState("");
  const [bulkResult, setBulkResult] = useState<number | null>(null);
  const policyParam = params.get("policy") === "1";
  const [policyOpen, setPolicyOpen] = useState(policyParam);
  useEffect(() => { if (policyParam) setPolicyOpen(true); }, [policyParam]);
  const flows = useQuery({ queryKey: ["inventory", "dataflows", "list"], queryFn: listDataflows });
  const applications = useQuery({ queryKey: ["inventory", "applications", "list"], queryFn: () => listApplications() });
  const groups = useQuery({ queryKey: ["inventory", "dataflow-groups", "list"], queryFn: () => listDataflowGroups() });
  const tags = useQuery(inventoryResourceOptionsQuery("Tags"));
  const applicationNames = useMemo(() => new Map((applications.data ?? []).map(item => [item.id, item.name])), [applications.data]);
  const groupNames = useMemo(() => new Map((groups.data ?? []).map(item => [item.id, item.name])), [groups.data]);
  const groupFilterIds = useMemo(() => group ? dataflowGroupDescendants(groups.data ?? [], group) : null, [group, groups.data]);
  const tagNames = useMemo(() => new Map((tags.data?.items ?? []).map(item => [item.id, item.name])), [tags.data]);
  const invalidApplication = !!application && applications.isSuccess && !applicationNames.has(application);
  const invalidGroup = !!group && groups.isSuccess && !groupNames.has(group);
  const invalidTag = !!tag && tags.isSuccess && !tagNames.has(tag);
  const invalidFilter = invalidTransport || invalidApplication || invalidGroup || invalidTag || (hasEndpointFilter && !validEndpointFilter);
  const activeFilterCount = [application, group, transport, tag, status !== "all" ? status : ""].filter(Boolean).length;
  const filterControls = <div className="flex flex-col gap-3 md:flex-row md:flex-wrap md:items-center">
    <FilterSelect label={t("dataflows.applicationFilter")} value={application} onChange={value => changeParams({ application: value || null })}>
      <option value="">{t("dataflows.allApplications")}</option>
      {(applications.data ?? []).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
    </FilterSelect>
    <FilterSelect label={t("dataflows.groupFilter")} value={group} onChange={value => changeParams({ group: value || null })}>
      <option value="">{t("dataflows.allGroups")}</option>
      {(groups.data ?? []).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
    </FilterSelect>
    <FilterSelect label={t("dataflows.transportFilter")} value={transport} onChange={value => changeParams({ transport: value || null })}>
      <option value="">{t("dataflows.allTransports")}</option>
      {TRANSPORTS.map(value => <option key={value} value={value}>{transportLabel(value, t)}</option>)}
    </FilterSelect>
    <FilterSelect label={t("dataflows.effectiveStatusFilter")} value={status} onChange={value => changeParams({ status: value === "all" ? null : value })}>
      <option value="all">{t("dataflows.allStatuses")}</option>
      {STATUSES.map(value => <option key={value} value={value}>{t(`dataflows.statuses.${value}`)}</option>)}
    </FilterSelect>
    <FilterSelect label={t("dataflows.tagFilter")} value={tag} onChange={value => changeParams({ tag: value || null })}>
      <option value="">{t("dataflows.allTags")}</option>
      {(tags.data?.items ?? []).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
    </FilterSelect>
    {activeFilterCount > 0 && <Button size="sm" variant="ghost" className="self-start md:self-auto" onClick={() => changeParams({ application: null, group: null, transport: null, status: null, tag: null })}>{t("dataflows.resetFilters")}</Button>}
  </div>;
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ["inventory", "dataflows"] });
    await qc.invalidateQueries({ queryKey: ["inventory", "dataflow-policy"] });
  };
  const disable = useApiMutation({ mutationFn: (flow: Dataflow) => updateDataflow(flow.id, { status: "disabled" }), onSuccess: refresh });
  const remove = useApiMutation({ mutationFn: deleteDataflow, onSuccess: async () => { setTarget(null); await refresh(); } });
  const bulk = useApiMutation({
    mutationFn: (body: DataflowBulkAction) => bulkDataflows(body),
    onSuccess: async result => {
      setBulkResult(result.affected.length);
      setSelectedKeys(new Set());
      setMoveOpen(false);
      setDeleteOpen(false);
      setMoveGroup("");
      setMoveApplication("");
      changeParams({ flow: null }, true);
      await refresh();
    },
  });
  const runBulk = (body: DataflowBulkAction) => { bulk.reset(); setBulkResult(null); bulk.mutate(body); };
  const selectedIds = useMemo(() => Array.from(selectedKeys), [selectedKeys]);
  const bulkTooLarge = selectedIds.length > 1000;
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");
  const canCreate = can("CREATE", "INVENTORY");
  const selectable = canUpdate || canDelete;
  const rows = (flows.data ?? []).filter(flow =>
    (status === "all" || flow.effective_status === status) &&
    (!application || flow.application_id === application) &&
    (!group || !!groupFilterIds?.has(flow.group_id ?? "")) &&
    (!transport || flow.transport === transport) &&
    (!tag || flow.effective_tags.includes(tag)) &&
    (!hasEndpointFilter || validEndpointFilter && (endpointSide === "source" ? flow.sources : endpointSide === "destination" ? flow.destinations : dataflowEndpoints(flow))
      .some(endpoint => endpoint.object_type === endpointType && endpoint.object_id === endpointId && (!endpointTagObjectType || endpoint.object_type === "extras.tag" && endpoint.tag_object_type === endpointTagObjectType))) &&
    [flow.name, ...dataflowEndpoints(flow).map(endpoint => endpointDisplayName(endpoint, t))].some(value => value.toLocaleLowerCase().includes(search.toLocaleLowerCase())));
  const selectedId = params.get("flow");
  const selected = flows.data?.find(flow => flow.id === selectedId);
  const filteredEndpoint = (flows.data ?? []).flatMap(dataflowEndpoints).find(endpoint => endpoint.object_type === endpointType && endpoint.object_id === endpointId);
  const blockedTitle = (flow: Dataflow) => flow.blocked_by_group_id
    ? t("dataflows.blockedByGroup", { name: groupNames.get(flow.blocked_by_group_id) ?? t("dataflows.unresolved") })
    : undefined;
  const inspect = (flow: Dataflow, opener: HTMLElement) => {
    previewFocus.capture(opener);
    disable.reset();
    changeParams({ flow: flow.id });
  };
  const renderActions = (flow: Dataflow) => canUpdate || canDelete || canCreate ? <RowActions label={t("dataflows.flowActions", { name: flow.name })} {...triggerProps(flow.id)}>
      {(canUpdate || canCreate) && <>
        {canUpdate && <DropdownMenuItem asChild><Link to={`/inventory/dataflows/${flow.id}/edit`}>{t("edit")}</Link></DropdownMenuItem>}
        {canCreate && <DropdownMenuItem asChild><Link to={`/inventory/dataflows/new?clone=${flow.id}`}>{t("dataflows.cloneFlow")}</Link></DropdownMenuItem>}
        {canUpdate && flow.effective_status !== "disabled" && <DropdownMenuItem disabled={disable.isPending} onSelect={() => disable.mutate(flow)}>{t("dataflows.disable")}</DropdownMenuItem>}
      </>}
      {canDelete && <>
        {canUpdate && <DropdownMenuSeparator />}
        <DropdownMenuItem variant="destructive" disabled={remove.isPending} onSelect={() => { remove.reset(); setTarget(flow); }}>{t("delete")}</DropdownMenuItem>
      </>}
  </RowActions> : null;
  const columns: Column<Dataflow>[] = [
    { key: "flow", label: t("name"), render: flow => <span className="flex min-w-0 flex-col gap-0.5">
      <button type="button" aria-haspopup="dialog" aria-label={t("dataflows.inspectFlow", { name: flow.name })} onClick={event => inspect(flow, event.currentTarget)} className="w-fit rounded-sm text-left text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{flow.name}</button>
      {[flow.application_id ? applicationNames.get(flow.application_id) : undefined, flow.group_id ? groupNames.get(flow.group_id) : undefined].filter(Boolean).length > 0 && <span className="break-words text-xs text-muted-foreground">
        {[flow.application_id ? applicationNames.get(flow.application_id) : undefined, flow.group_id ? groupNames.get(flow.group_id) : undefined].filter(Boolean).join(" · ")}
      </span>}
    </span> },
    { key: "source", label: t("dataflows.source"), render: flow => <EndpointSummary endpoints={flow.sources} /> },
    { key: "destination", label: t("dataflows.destination"), render: flow => <EndpointSummary endpoints={flow.destinations} /> },
    { key: "traffic", label: t("dataflows.transport"), render: flow => <FlowTraffic flow={flow} compact /> },
    { key: "status", label: t("dataflows.effectiveStatus"), render: flow => <FlowStatus flow={flow} title={blockedTitle(flow)} /> },
    { key: "actions", label: t("actions"), excludeFromSettings: true, render: renderActions },
  ];
  const moveBody = (): DataflowBulkAction | null => {
    if (moveGroup === "" && moveApplication === "") return null;
    const body: DataflowBulkAction = { ids: selectedIds, action: "move" };
    if (moveGroup !== "") body.group_id = moveGroup === CLEAR_VALUE ? null : moveGroup;
    if (moveApplication !== "") body.application_id = moveApplication === CLEAR_VALUE ? null : moveApplication;
    return body;
  };
  return <div className="flex min-w-0 flex-col gap-4">
    <DataflowCatalogNav />
    <PageChrome title={t("dataflows.title")} description={t("dataflows.description")} icon={Network} actions={<>
      <Sheet open={policyOpen} onOpenChange={open => { setPolicyOpen(open); if (!open && policyParam) changeParams({ policy: null }, true); }}><SheetTrigger asChild><Button variant="outline"><Download aria-hidden="true" className="size-4" />{t("dataflows.exportPolicy")}</Button></SheetTrigger>
        <SheetContent className="flex w-full flex-col gap-6 overflow-y-auto sm:max-w-2xl"><SheetHeader><SheetTitle>{t("dataflows.policyTitle")}</SheetTitle><SheetDescription>{t("dataflows.policyHint")}</SheetDescription></SheetHeader><DataflowPolicyPanel /></SheetContent>
      </Sheet>
      {canCreate && <Button asChild><Link to="/inventory/dataflows/new"><Plus aria-hidden="true" className="size-4" />{t("dataflows.newTitle")}</Link></Button>}
    </>} />
    {(disable.isError || remove.isError) && <p role="alert" className="text-destructive">{t("dataflows.writeFailed")}</p>}
    {(invalidView || invalidStatus) && <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">{t("dataflows.invalidViewFilter")}<Button variant="outline" size="sm" onClick={() => changeParams({ view: "list", status: "all" })}>{t("dataflows.resetViewFilters")}</Button></div>}
    {invalidFilter && <div role="alert" className="flex flex-wrap items-center gap-2 text-sm text-destructive">{t("dataflows.invalidFilter", { defaultValue: t("dataflows.invalidViewFilter") })}<Button variant="outline" size="sm" onClick={() => changeParams({ application: null, group: null, transport: null, status: null, tag: null, endpointType: null, endpointId: null, endpointSide: null, endpointTagObjectType: null })}>{t("dataflows.resetViewFilters")}</Button></div>}
    <SurfaceCard className="flex flex-col gap-2 p-3 sm:p-4">
      <SectionLinks label={t("dataflows.viewLabel")} activeValue={view} revealActive={false} className="w-auto max-w-full border-0" items={[
        { value: "list", label: t("dataflows.listView"), icon: List }, { value: "overview", label: t("dataflows.groupedView"), icon: Waypoints }, { value: "topology", label: t("dataflows.topologyView"), icon: Network }, { value: "matrix", label: t("dataflows.matrixView"), icon: Grid2X2 },
      ].map(item => ({ ...item, to: { pathname: location.pathname, search: `?${updatedParams({ view: item.value })}`, hash: location.hash } }))} />
      <div className="flex flex-wrap items-center gap-2">
        <SearchInput ref={searchInput} value={search} onValueChange={value => changeParams({ q: value }, true)} placeholder={t("dataflows.search")} containerClassName="min-w-40 grow basis-40 sm:min-w-0 sm:grow-0 sm:basis-72" />
        <Popover>
          <PopoverTrigger asChild>
            <Button variant="outline" size="sm" className="md:hidden" aria-label={activeFilterCount > 0 ? t("dataflows.filtersWithCount", { count: activeFilterCount }) : t("dataflows.filters")}>
              <SlidersHorizontal aria-hidden="true" className="size-4" />{t("dataflows.filters")}
              {activeFilterCount > 0 && <span className="rounded-full bg-primary px-1.5 text-[11px] font-semibold leading-5 text-primary-foreground">{activeFilterCount}</span>}
            </Button>
          </PopoverTrigger>
          <PopoverContent align="start" aria-label={t("dataflows.filters")} className="w-[min(20rem,calc(100vw-2rem))] p-3">{filterControls}</PopoverContent>
        </Popover>
        <div className="hidden md:block">{filterControls}</div>
      </div>
      {hasEndpointFilter && <div className="flex flex-wrap items-center justify-between gap-2">
      <div className="flex min-w-0 flex-col gap-1"><p className="break-words text-sm font-medium">{validEndpointFilter ? t("dataflows.endpointFilter", { name: filteredEndpoint?.label ?? endpointId }) : t("dataflows.invalidEndpointFilter")}</p><p className="text-xs text-muted-foreground">{t("dataflows.exactEndpointHint")}</p></div>
      <div className="flex flex-wrap items-center gap-2">
        {endpointType === "extras.tag" && <FilterSelect label={t("dataflows.tagScope")} value={endpointTagObjectType} onChange={value => changeParams({ endpointTagObjectType: value || null })}>
          <option value="">{t("dataflows.allScopes")}</option>
          <option value="dcim.device">{t("dataflows.tagScopeDevice")}</option>
          <option value="virtualization.virtualmachine">{t("dataflows.tagScopeVm")}</option>
        </FilterSelect>}
        <FilterSelect label={t("dataflows.endpointSide")} value={endpointSide} onChange={value => changeParams({ endpointSide: value === "any" ? null : value })}>
          <option value="any">{t("dataflows.allSides")}</option>
          <option value="source">{t("dataflows.source")}</option>
          <option value="destination">{t("dataflows.destination")}</option>
        </FilterSelect>
        <Button variant="outline" size="sm" onClick={() => changeParams({ endpointType: null, endpointId: null, endpointSide: null, endpointTagObjectType: null })}>{t("dataflows.clearEndpointFilter")}</Button>
      </div>
      </div>}
      {!flows.isPending && !flows.isError && <div className="flex flex-wrap items-center gap-x-3 gap-y-1"><span className="text-xs tabular-nums text-muted-foreground">{t("dataflows.flowCount", { count: rows.length })}</span><FlowStatusSummary flows={rows} /></div>}
    </SurfaceCard>
      {selectedId && !flows.isPending && !flows.isError && !selected && <div className="flex items-center gap-2 text-sm text-muted-foreground"><output>{t("dataflows.selectionUnavailable")}</output><Button variant="ghost" size="sm" onClick={() => changeParams({ flow: null })}>{t("close")}</Button></div>}
      {bulkResult !== null && <output role="status" className="text-sm text-muted-foreground">{t("dataflows.bulkSucceeded", { count: bulkResult })}</output>}
      {selectable && selectedKeys.size > 0 && <SurfaceCard role="region" aria-label={t("dataflows.bulkSelected", { count: selectedIds.length })} className="flex flex-wrap items-center gap-2 p-3">
        <span className="text-sm font-medium">{t("dataflows.bulkSelected", { count: selectedIds.length })}</span>
        {canUpdate && <Button size="sm" disabled={bulk.isPending || bulkTooLarge} onClick={() => runBulk({ ids: selectedIds, action: "set_status", status: "active" })}>{t("dataflows.bulkActivate")}</Button>}
        {canUpdate && <Button size="sm" variant="outline" disabled={bulk.isPending || bulkTooLarge} onClick={() => runBulk({ ids: selectedIds, action: "set_status", status: "disabled" })}>{t("dataflows.bulkDisable")}</Button>}
        {canUpdate && <Button size="sm" variant="outline" disabled={bulk.isPending || bulkTooLarge} onClick={() => { bulk.reset(); setMoveOpen(true); }}>{t("dataflows.bulkMove")}</Button>}
        {canDelete && <Button size="sm" variant="destructive" disabled={bulk.isPending || bulkTooLarge} onClick={() => { bulk.reset(); setDeleteOpen(true); }}>{t("dataflows.bulkDelete")}</Button>}
        <Button size="sm" variant="ghost" onClick={() => setSelectedKeys(new Set())}>{t("clear")}</Button>
      </SurfaceCard>}
      {bulkTooLarge && <p role="alert" className="text-sm text-destructive">{t("dataflows.bulkLimit", { count: 1000, defaultValue: "Select no more than 1000 flows for one bulk action." })}</p>}
      {bulk.isError && !moveOpen && !deleteOpen && <p role="alert" className="text-sm text-destructive">{t("dataflows.bulkFailed")}</p>}
      {view === "list" ? <>
        {flows.isPending ? <SurfaceCard><PageState kind="loading" title={t("dataflows.overviewLoading")} /></SurfaceCard>
          : flows.isError ? <SurfaceCard><PageState kind="error" title={t("dataflows.loadFailed")} action={<Button variant="outline" onClick={() => void flows.refetch()}>{t("retry")}</Button>} /></SurfaceCard>
          : <DataTable<Dataflow> data={rows} columns={columns} getRowKey={flow => flow.id} tableId="dataflow-catalog" selectable={selectable} selectedKeys={selectedKeys} onSelectionChange={setSelectedKeys} selectedRow={selected ?? null} onRowClick={(flow, row) => inspect(flow, row)} emptyState={<PageState kind="empty" title={t("dataflows.empty")} />} />}
        <DataflowDetail flow={selected} renderActions={canUpdate || canDelete || canCreate ? renderActions : undefined} writeFailed={disable.isError || remove.isError} onClose={() => { disable.reset(); changeParams({ flow: null }); }} onCloseAutoFocus={previewFocus.onCloseAutoFocus} />
      </> : <DataflowOverview anchor={searchInput} flows={rows} loading={flows.isPending} failed={flows.isError} onRetry={() => void flows.refetch()} renderActions={canUpdate || canDelete || canCreate ? renderActions : undefined} view={view} selected={selected} writeFailed={disable.isError || remove.isError} onSelect={id => { disable.reset(); changeParams({ flow: id }); }} />}
    <Dialog open={moveOpen} onOpenChange={open => { if (!open && !bulk.isPending) { setMoveOpen(false); bulk.reset(); } }}>
      <DialogContent description={t("dataflows.bulkMoveDescription", { count: selectedIds.length })}>
        <DialogHeader><DialogTitle>{t("dataflows.bulkMoveTitle")}</DialogTitle><DialogDescription>{t("dataflows.bulkMoveDescription", { count: selectedIds.length })}</DialogDescription></DialogHeader>
        <div className="grid gap-3">
          <label className="flex min-w-0 flex-col gap-1 text-sm"><span className="font-medium">{t("dataflows.bulkGroup")}</span><select aria-label={t("dataflows.bulkGroup")} value={moveGroup} onChange={event => setMoveGroup(event.target.value)} className="control-surface h-9 rounded-md px-2 text-sm">
            <option value="">{t("dataflows.bulkKeep", { defaultValue: "Keep current" })}</option>
            <option value={CLEAR_VALUE}>{t("dataflows.noGroup")}</option>
            {(groups.data ?? []).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select></label>
          <label className="flex min-w-0 flex-col gap-1 text-sm"><span className="font-medium">{t("dataflows.bulkApplication")}</span><select aria-label={t("dataflows.bulkApplication")} value={moveApplication} onChange={event => setMoveApplication(event.target.value)} className="control-surface h-9 rounded-md px-2 text-sm">
            <option value="">{t("dataflows.bulkKeep", { defaultValue: "Keep current" })}</option>
            <option value={CLEAR_VALUE}>{t("dataflows.noApplication")}</option>
            {(applications.data ?? []).map(item => <option key={item.id} value={item.id}>{item.name}</option>)}
          </select></label>
        </div>
        {bulk.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.bulkFailed")}</p>}
        <DialogFooter>
          <Button variant="outline" onClick={() => { setMoveOpen(false); bulk.reset(); }} disabled={bulk.isPending}>{t("cancel")}</Button>
          <Button disabled={bulk.isPending || moveBody() === null} onClick={() => { const body = moveBody(); if (body) runBulk(body); }}>{t("dataflows.bulkMove")}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
    <AlertDialog open={deleteOpen} onOpenChange={open => { if (!open && !bulk.isPending) { setDeleteOpen(false); bulk.reset(); } }}>
      <AlertDialogContent><AlertDialogHeader><AlertDialogTitle>{t("dataflows.bulkDelete")}</AlertDialogTitle>
        <AlertDialogDescription>{t("dataflows.bulkSelected", { count: selectedIds.length })}</AlertDialogDescription></AlertDialogHeader>
        {bulk.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.bulkFailed")}</p>}
        <AlertDialogFooter><AlertDialogCancel disabled={bulk.isPending}>{t("cancel")}</AlertDialogCancel>
          <AlertDialogAction disabled={bulk.isPending || bulkTooLarge} onClick={event => { event.preventDefault(); runBulk({ ids: selectedIds, action: "delete" }); }}>{t("delete")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
    <AlertDialog open={!!target} onOpenChange={open => { if (!open && !remove.isPending) setTarget(null); }}>
      <AlertDialogContent onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}><AlertDialogHeader><AlertDialogTitle>{t("dataflows.deleteTitle")}</AlertDialogTitle>
        <AlertDialogDescription>{t("dataflows.deleteDescription", { name: target?.name })}</AlertDialogDescription></AlertDialogHeader>
        {remove.isError && <p role="alert" className="text-destructive">{t("dataflows.writeFailed")}</p>}
        <AlertDialogFooter><AlertDialogCancel disabled={remove.isPending}>{t("cancel")}</AlertDialogCancel>
          <AlertDialogAction disabled={remove.isPending} onClick={event => { event.preventDefault(); if (target && canDelete) remove.mutate(target.id); }}>{t("delete")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>;
}
