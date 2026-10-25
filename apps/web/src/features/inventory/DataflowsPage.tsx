import { useRef, useState } from "react";
import { Link, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Download, List, Network, Plus } from "lucide-react";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { Button } from "@/shared/ui-primitives/button";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import PageChrome from "@/shared/ui-shared/PageChrome";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle, SheetTrigger } from "@/shared/ui-primitives/sheet";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import SearchInput from "@/shared/ui-shared/SearchInput";
import SectionTabs from "@/shared/ui-shared/SectionTabs";
import { Tabs, TabsContent } from "@/shared/ui-primitives/tabs";
import { DATAFLOW_ENDPOINTS } from "./dataflowEndpoints";
import { deleteDataflow, listDataflows, updateDataflow } from "./dataflows-api";
import type { Dataflow } from "./dataflows-api";
import DataflowPolicyPanel from "./DataflowPolicyPanel";
import DataflowOverview from "./DataflowOverview";

export default function DataflowsPage() {
  const { t } = useTranslation();
  const searchInput = useRef<HTMLInputElement>(null);
  const { can } = usePermissions();
  const qc = useQueryClient();
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const [params, setParams] = useSearchParams();
  const search = params.get("q") ?? "";
  const view = params.get("view") === "topology" ? "topology" : "list";
  const status = params.get("status") ?? "all";
  const endpointType = params.get("endpointType");
  const endpointId = params.get("endpointId");
  const hasEndpointFilter = endpointType !== null || endpointId !== null;
  const validEndpointFilter = !!endpointType && Object.hasOwn(DATAFLOW_ENDPOINTS, endpointType) && !!endpointId;
  const changeParams = (changes: Record<string, string | null>, replace = false) => setParams(previous => {
    const next = new URLSearchParams(previous);
    for (const [key, value] of Object.entries(changes)) { if (value) next.set(key, value); else next.delete(key); }
    return next;
  }, { replace });
  const [target, setTarget] = useState<Dataflow | null>(null);
  const flows = useQuery({ queryKey: ["inventory", "dataflows", "list"], queryFn: listDataflows });
  const refresh = async () => {
    await qc.invalidateQueries({ queryKey: ["inventory", "dataflows"] });
    await qc.invalidateQueries({ queryKey: ["inventory", "dataflow-policy"] });
  };
  const disable = useApiMutation({ mutationFn: (flow: Dataflow) => updateDataflow(flow.id, { status: "disabled" }), onSuccess: refresh });
  const remove = useApiMutation({ mutationFn: deleteDataflow, onSuccess: async () => { setTarget(null); await refresh(); } });
  const endpointLabel = (endpoint: Dataflow["source"]) => endpoint.object_type === "all" || endpoint.object_type === "internet"
    ? t(`dataflows.endpointTypes.${endpoint.object_type}`) : endpoint.label ?? t("dataflows.unresolved");
  const rows = (flows.data ?? []).filter(flow =>
    (status === "all" || flow.status === status) &&
    (!hasEndpointFilter || validEndpointFilter && [flow.source, flow.destination].some(endpoint => endpoint.object_type === endpointType && endpoint.object_id === endpointId)) &&
    [flow.name, endpointLabel(flow.source), endpointLabel(flow.destination)].some(value => value.toLocaleLowerCase().includes(search.toLocaleLowerCase())));
  const selectedId = params.get("flow");
  const selected = flows.data?.find(flow => flow.id === selectedId);
  const filteredEndpoint = (flows.data ?? []).flatMap(flow => [flow.source, flow.destination]).find(endpoint => endpoint.object_type === endpointType && endpoint.object_id === endpointId);
  const renderActions = (flow: Dataflow) => can("UPDATE", "INVENTORY") || can("DELETE", "INVENTORY") ? <RowActions label={t("dataflows.flowActions", { name: flow.name })} {...triggerProps(flow.id)}>
      {can("UPDATE", "INVENTORY") && <>
        <DropdownMenuItem asChild><Link to={`/inventory/dataflows/${flow.id}/edit`}>{t("edit")}</Link></DropdownMenuItem>
        {flow.status !== "disabled" && <DropdownMenuItem disabled={disable.isPending} onSelect={() => disable.mutate(flow)}>{t("dataflows.disable")}</DropdownMenuItem>}
      </>}
      {can("DELETE", "INVENTORY") && <>
        {can("UPDATE", "INVENTORY") && <DropdownMenuSeparator />}
        <DropdownMenuItem variant="destructive" disabled={remove.isPending} onSelect={() => { remove.reset(); setTarget(flow); }}>{t("delete")}</DropdownMenuItem>
      </>}
  </RowActions> : null;
  return <div className="flex min-w-0 flex-col gap-6">
    <PageChrome title={t("dataflows.title")} description={t("dataflows.description")} icon={Network} actions={<>
      <Sheet><SheetTrigger asChild><Button variant="outline"><Download aria-hidden="true" className="size-4" />{t("dataflows.exportPolicy")}</Button></SheetTrigger>
        <SheetContent className="flex w-full flex-col gap-6 overflow-y-auto sm:max-w-2xl"><SheetHeader><SheetTitle>{t("dataflows.policyTitle")}</SheetTitle><SheetDescription>{t("dataflows.policyHint")}</SheetDescription></SheetHeader><DataflowPolicyPanel /></SheetContent>
      </Sheet>
      {can("CREATE", "INVENTORY") && <Button asChild><Link to="/inventory/dataflows/new"><Plus aria-hidden="true" className="size-4" />{t("dataflows.newTitle")}</Link></Button>}
    </>} />
    {(disable.isError || remove.isError) && <p role="alert" className="text-destructive">{t("dataflows.writeFailed")}</p>}
    {hasEndpointFilter && <div className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-border bg-muted/40 px-4 py-3">
      <div className="flex min-w-0 flex-col gap-1"><p className="break-words text-sm font-medium">{validEndpointFilter ? t("dataflows.endpointFilter", { name: filteredEndpoint?.label ?? endpointId }) : t("dataflows.invalidEndpointFilter")}</p><p className="text-xs text-muted-foreground">{t("dataflows.exactEndpointHint")}</p></div>
      <Button variant="outline" size="sm" onClick={() => changeParams({ endpointType: null, endpointId: null })}>{t("dataflows.clearEndpointFilter")}</Button>
    </div>}
    <Tabs value={view} onValueChange={value => changeParams({ view: value })} className="flex min-w-0 flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-4"><SectionTabs aria-label={t("dataflows.viewLabel")} items={[{ value: "list", label: t("dataflows.listView"), icon: List }, { value: "topology", label: t("dataflows.topologyView"), icon: Network }]} /><span className="text-xs tabular-nums text-muted-foreground">{t("dataflows.flowCount", { count: rows.length })}</span></div>
      <div className="flex flex-wrap items-center gap-4"><SearchInput ref={searchInput} value={search} onValueChange={value => changeParams({ q: value }, true)} placeholder={t("dataflows.search")} /><label className="flex items-center gap-2 text-xs text-muted-foreground">{t("dataflows.status")}<select aria-label={t("dataflows.status")} value={status} onChange={event => changeParams({ status: event.target.value })} className="rounded-md border border-input bg-background p-2 text-sm text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"><option value="all">{t("dataflows.allStatuses")}</option>{["active", "draft", "disabled"].map(value => <option key={value} value={value}>{t(`dataflows.statuses.${value}`)}</option>)}</select></label></div>
      {selectedId && !flows.isPending && !flows.isError && !selected && <div className="flex items-center gap-2 text-sm text-muted-foreground"><output>{t("dataflows.selectionUnavailable")}</output><Button variant="ghost" size="sm" onClick={() => changeParams({ flow: null })}>{t("close")}</Button></div>}
      {(["list", "topology"] as const).map(mode => <TabsContent key={mode} value={mode} className="mt-0"><DataflowOverview anchor={searchInput} flows={rows} loading={flows.isPending} failed={flows.isError} onRetry={() => void flows.refetch()} renderActions={can("UPDATE", "INVENTORY") || can("DELETE", "INVENTORY") ? renderActions : undefined} view={mode} selected={selected} writeFailed={disable.isError || remove.isError} onSelect={id => { disable.reset(); changeParams({ flow: id }); }} /></TabsContent>)}
    </Tabs>
    <AlertDialog open={!!target} onOpenChange={open => { if (!open && !remove.isPending) setTarget(null); }}>
      <AlertDialogContent onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}><AlertDialogHeader><AlertDialogTitle>{t("dataflows.deleteTitle")}</AlertDialogTitle>
        <AlertDialogDescription>{t("dataflows.deleteDescription", { name: target?.name })}</AlertDialogDescription></AlertDialogHeader>
        {remove.isError && <p role="alert" className="text-destructive">{t("dataflows.writeFailed")}</p>}
        <AlertDialogFooter><AlertDialogCancel disabled={remove.isPending}>{t("cancel")}</AlertDialogCancel>
          <AlertDialogAction disabled={remove.isPending} onClick={event => { event.preventDefault(); if (target && can("DELETE", "INVENTORY")) remove.mutate(target.id); }}>{t("delete")}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  </div>;
}
