import { useTranslation } from "react-i18next";
import { Link, useLocation } from "react-router";
import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "@/shared/ui-primitives/sheet";
import type { Dataflow, DataflowEndpointResponse } from "./dataflows-api";
import { FlowStatus, FlowTraffic, endpointDisplayName } from "./DataflowConnection";
import DataflowEndpointMembers from "./DataflowEndpointMembers";
import { dataflowEndpointInput, dataflowEndpointKey } from "./dataflowEndpoints";
import { listApplications, listDataflowGroups } from "./dataflowCatalog-api";
import { inventoryResourceOptionsQuery } from "./resourceOptions";

function lookupName(items: readonly { id: string; name: string }[] | undefined, id: string | null | undefined, pending: boolean, unresolved: string, loading: string): string | null {
  if (!id) return null;
  if (pending) return loading;
  return items?.find(item => item.id === id)?.name ?? unresolved;
}

function TagList({ ids, label, tags, pending }: Readonly<{
  ids: readonly string[];
  label: string;
  tags: ReadonlyMap<string, string>;
  pending: boolean;
}>) {
  const { t } = useTranslation();
  return <section className="flex min-w-0 flex-col gap-1">
    <h3 className="text-sm font-semibold">{label}</h3>
    {ids.length > 0 ? <ul className="flex flex-wrap gap-1.5" aria-label={label}>
      {ids.map(id => <li key={id} className="surface-inset rounded-md px-2 py-0.5 text-xs">{tags.get(id) ?? (pending ? t("loading") : t("dataflows.unresolved"))}</li>)}
    </ul> : <p className="text-xs text-muted-foreground">{t("dataflows.noTags")}</p>}
  </section>;
}

function EndpointSide({ label, endpoints, flowId }: Readonly<{ label: string; endpoints: DataflowEndpointResponse[]; flowId: string }>) {
  const { t } = useTranslation();
  return <section aria-label={label} className="flex min-w-0 flex-col gap-2 border-t border-border pt-4">
    <h2 className="text-sm font-semibold">{label}</h2>
    {endpoints.length > 0 ? endpoints.map(endpoint => <DataflowEndpointMembers key={`${flowId}:${dataflowEndpointKey(endpoint)}`} value={dataflowEndpointInput(endpoint)} label={endpointDisplayName(endpoint, t)} />)
      : <p className="text-sm text-muted-foreground">{t("dataflows.endpointRequired")}</p>}
  </section>;
}

export default function DataflowDetail({ flow, onClose, onCloseAutoFocus, renderActions, writeFailed }: Readonly<{
  flow?: Dataflow; onClose: () => void; onCloseAutoFocus: (event: Event) => void; renderActions?: (flow: Dataflow) => ReactNode; writeFailed: boolean;
}>) {
  const { t } = useTranslation();
  const location = useLocation();
  const applications = useQuery({ queryKey: ["inventory", "applications", "list"], queryFn: () => listApplications(), enabled: !!flow?.application_id });
  const groups = useQuery({
    queryKey: ["inventory", "dataflow-groups", "list"],
    queryFn: () => listDataflowGroups(),
    enabled: !!flow && (!!flow.group_id || !!flow.blocked_by_group_id),
  });
  const tags = useQuery({ ...inventoryResourceOptionsQuery("Tags"), enabled: !!flow && (flow.tags.length > 0 || flow.inherited_tags.length > 0) });
  const tagNames = new Map((tags.data?.items ?? []).map(item => [item.id, item.name]));
  const applicationName = lookupName(applications.data, flow?.application_id, applications.isPending, t("dataflows.unresolved"), t("loading"));
  const groupName = lookupName(groups.data, flow?.group_id, groups.isPending, t("dataflows.unresolved"), t("loading"));
  const blockedGroupName = lookupName(groups.data, flow?.blocked_by_group_id, groups.isPending, t("dataflows.unresolved"), t("loading"));
  const policySearch = new URLSearchParams(location.search);
  policySearch.delete("flow");
  policySearch.set("policy", "1");
  return <Sheet open={!!flow} onOpenChange={open => { if (!open) onClose(); }}>
    <SheetContent className="flex w-full flex-col gap-6 overflow-y-auto text-foreground sm:max-w-xl" onCloseAutoFocus={onCloseAutoFocus}>
      {flow && <>
        <SheetHeader><SheetTitle className="break-words">{flow.name}</SheetTitle><SheetDescription>{t("dataflows.detailHint")}</SheetDescription></SheetHeader>
        <div className="flex flex-wrap items-center gap-2"><FlowTraffic flow={flow} />{renderActions?.(flow)}</div>
        {writeFailed && <p role="alert" className="text-sm text-destructive">{t("dataflows.writeFailed")}</p>}
        <dl className="grid min-w-0 gap-2 text-sm">
          <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2">
            <dt className="text-muted-foreground">{t("dataflows.effectiveStatus")}</dt>
            <dd><FlowStatus flow={flow} /></dd>
          </div>
          {applicationName && <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2">
            <dt className="text-muted-foreground">{t("dataflows.bulkApplication")}</dt>
            <dd className="min-w-0 break-words text-right">{applicationName}</dd>
          </div>}
          {groupName && <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2">
            <dt className="text-muted-foreground">{t("dataflows.bulkGroup")}</dt>
            <dd className="min-w-0 break-words text-right">{groupName}</dd>
          </div>}
        </dl>
        {blockedGroupName && <p role="status" className="rounded-md border border-border p-3 text-sm">{t("dataflows.blockedByGroup", { name: blockedGroupName })}</p>}
        <EndpointSide label={t("dataflows.sources")} endpoints={flow.sources} flowId={flow.id} />
        <EndpointSide label={t("dataflows.destinations")} endpoints={flow.destinations} flowId={flow.id} />
        <div className="grid min-w-0 gap-4 border-t border-border pt-4 @sm:grid-cols-2">
          <TagList label={t("dataflows.directTags")} ids={flow.tags} tags={tagNames} pending={tags.isPending} />
          <TagList label={t("dataflows.inheritedTags")} ids={flow.inherited_tags} tags={tagNames} pending={tags.isPending} />
        </div>
        {flow.comments && <section className="flex flex-col gap-1 border-t border-border pt-4">
          <h3 className="text-sm font-semibold">{t("dataflows.comments", { defaultValue: "Comments" })}</h3>
          <p className="whitespace-pre-wrap break-words text-sm text-muted-foreground">{flow.comments}</p>
        </section>}
        {flow.description && <details className="border-t border-border pt-4"><summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("dataflows.evidence")}</summary><p className="mt-4 whitespace-pre-wrap break-words text-sm text-muted-foreground">{flow.description}</p></details>}
        <Link className="w-fit rounded-sm text-sm font-medium text-primary underline-offset-4 hover:underline focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring" to={{ pathname: "/inventory/dataflows", search: `?${policySearch}`, hash: location.hash }} onClick={onClose}>{t("dataflows.policyTitle")}</Link>
      </>}
    </SheetContent>
  </Sheet>;
}
