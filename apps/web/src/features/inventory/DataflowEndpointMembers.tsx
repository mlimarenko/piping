import { useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Server, Globe, Users } from "lucide-react";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { useDataflowEndpointResolution } from "./useDataflowEndpointResolution";
import type { DataflowEndpoint, DataflowEndpointResolution } from "./dataflows-api";

function dataflowDiagnosticKey(item: { code: string; message: string; dataflow_id?: string | null }) {
  return JSON.stringify([item.code, item.message, item.dataflow_id ?? null]);
}

function uniqueDataflowDiagnostics<T extends { code: string; message: string; dataflow_id?: string | null }>(items: readonly T[]): T[] {
  return [...new Map(items.map(item => [dataflowDiagnosticKey(item), item])).values()];
}

function addressKey(address: DataflowEndpointResolution["addresses"][number]) {
  return JSON.stringify([address.cidr, address.vrf_id]);
}

export function DataflowDiagnostics({ items, policy = false }: Readonly<{
  items: readonly { code: string; message: string; dataflow_id?: string | null }[];
  policy?: boolean;
}>) {
  const { t } = useTranslation();
  return <ul aria-label={t("dataflows.diagnostics")} className="flex flex-col gap-2">
    {uniqueDataflowDiagnostics(items).map(item => <li key={dataflowDiagnosticKey(item)} className={policy ? "break-words rounded-md border border-border p-4 text-sm" : "rounded-md bg-muted p-2 text-sm text-muted-foreground"}>
      {t(`dataflows.diagnosticCodes.${item.code}`, { defaultValue: t("dataflows.unresolvedDiagnostic") })}
      {policy && item.dataflow_id && <span className="block text-xs text-muted-foreground">{t("dataflows.provenance")}: {item.dataflow_id}</span>}
    </li>)}
  </ul>;
}

export function DataflowMemberList({ data, label }: Readonly<{ data: DataflowEndpointResolution; label: string }>) {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const listId = useId();
  const query = search.trim().toLocaleLowerCase();
  const members = data.members.filter(member => [member.host.label ?? "", member.host.object_id, ...member.addresses.map(address => address.cidr)].some(text => text.toLocaleLowerCase().includes(query)));
  const diagnostic = (code: string) => t(`dataflows.diagnosticCodes.${code}`, { defaultValue: t("dataflows.unresolvedDiagnostic") });
  return <div className="flex min-w-0 flex-col gap-2">
    <p className="flex items-center gap-2 text-sm font-medium"><Users aria-hidden="true" className="size-4 text-primary" />{t("dataflows.memberCount", { count: data.members.length })}</p>
    {data.diagnostics.length > 0 && <DataflowDiagnostics items={data.diagnostics} />}
    {data.addresses.length > 0 && data.members.length !== 1 && <details className="rounded-md border border-border p-2 text-sm">
      <summary className="cursor-pointer rounded-sm focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("dataflows.resolvedAddresses", { count: data.addresses.length })}</summary>
      <ul className="mt-2 flex max-h-40 flex-col gap-1 overflow-y-auto">{data.addresses.map(address => <li key={addressKey(address)} className="break-all font-mono text-xs">{address.cidr} · {address.vrf_id ?? t("dataflows.defaultVrf")}</li>)}</ul>
    </details>}
    {data.members.length > 0 ? <>
      {data.members.length > 1 && <><SearchInput aria-label={t("dataflows.memberSearch", { label })} placeholder={t("dataflows.memberSearch", { label })} value={search} onValueChange={setSearch} aria-controls={listId} />
      <output className="text-xs text-muted-foreground">{t("dataflows.membersShown", { shown: members.length, total: data.members.length })}</output></>}
      <ul id={listId} aria-label={t("dataflows.membersLabel", { label })} tabIndex={0} className="flex max-h-72 flex-col gap-2 overflow-y-auto rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
        {members.map(member => <li key={`${member.host.object_type}:${member.host.object_id}`} className="flex min-w-0 flex-col gap-1 rounded-md bg-muted/40 p-2">
          <span className="flex items-start gap-2 text-sm font-medium"><Server aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" /><span title={member.host.object_id} className="break-words">{member.host.label ?? t("dataflows.unresolved")}</span></span>
          {member.addresses.length ? member.addresses.map(address => <span key={addressKey(address)} className="break-all font-mono text-xs">{address.cidr} · {address.vrf_id ?? t("dataflows.defaultVrf")}</span>) : <span className="text-xs text-muted-foreground">{t("dataflows.noMemberAddresses")}</span>}
          {uniqueDataflowDiagnostics(member.diagnostics).map(item => <span key={dataflowDiagnosticKey(item)} className="text-xs text-muted-foreground">{diagnostic(item.code)}</span>)}
        </li>)}
        {!members.length && <li className="p-2 text-sm text-muted-foreground">{t("dataflows.noMatchingMembers")}</li>}
      </ul>
    </> : <EmptyState icon={data.endpoint.object_type === "internet" ? Globe : Users} title={t(data.endpoint.object_type === "internet" ? "dataflows.internetMembers" : "dataflows.noMembers")} />}
  </div>;
}

export default function DataflowEndpointMembers({ value, label }: Readonly<{ value: DataflowEndpoint; label: string }>) {
  const { t } = useTranslation();
  const resolution = useDataflowEndpointResolution(value);
  let content;
  if (resolution.isPending) content = <output className="text-sm text-muted-foreground">{t("dataflows.membersLoading")}</output>;
  else if (resolution.isError) content = <div className="flex flex-col gap-2"><p role="alert" className="text-sm text-destructive">{t("dataflows.resolutionFailed")}</p><Button size="sm" variant="outline" onClick={() => void resolution.refetch()}>{t("retry")}</Button></div>;
  else content = <DataflowMemberList data={resolution.data} label={label} />;
  return <section aria-label={t("dataflows.membersLabel", { label })} className="flex min-w-0 flex-col gap-2 text-foreground">
    <h3 className="text-sm font-semibold">{label}</h3>
    {content}
  </section>;
}
