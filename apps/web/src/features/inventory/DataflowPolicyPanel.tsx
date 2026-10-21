import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import { Button } from "@/shared/ui-primitives/button";
import DataTable from "@/shared/ui-shared/DataTable";
import DataflowEndpointPicker from "./DataflowEndpointPicker";
import { DataflowDiagnostics } from "./DataflowEndpointMembers";
import { getDataflowPolicy } from "./dataflows-api";
import type { DataflowEndpoint, DataflowPolicy, DataflowPolicyHost } from "./dataflows-api";

export default function DataflowPolicyPanel() {
  const { t } = useTranslation();
  const [host, setHost] = useState<DataflowEndpoint>({ object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] });
  const [requested, setRequested] = useState<DataflowPolicyHost | null>(null);
  const policy = useQuery({
    queryKey: ["inventory", "dataflow-policy", requested],
    queryFn: () => {
      if (!requested) throw new Error("Select a policy host first");
      return getDataflowPolicy(requested);
    },
    enabled: !!requested,
    staleTime: 0,
  });
  const current = requested?.object_type === host.object_type && requested?.object_id === host.object_id;
  const data = current && !policy.isError ? policy.data : undefined;
  const exportPolicy = () => {
    if (!data?.apply_ready || policy.isFetching) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }));
    const link = document.createElement("a");
    link.href = url;
    link.download = `firewall-policy-${data.host.object_id}.json`;
    link.click();
    URL.revokeObjectURL(url);
  };
  type Rule = DataflowPolicy["rules"][number];
  return <section aria-label={t("dataflows.policyTitle")} className="flex min-w-0 shrink-0 flex-col gap-4">
    <div className="grid min-w-0 gap-4 md:grid-cols-[minmax(0,1fr)_auto]">
      <DataflowEndpointPicker label={t("dataflows.host")} value={host} onChange={value => { setHost(value); setRequested(null); }} hostsOnly />
      <div className="flex flex-wrap items-end gap-2">
        <Button disabled={!host.object_id || policy.isFetching} onClick={() => {
          if (!host.object_id || (host.object_type !== "dcim.device" && host.object_type !== "virtualization.virtualmachine")) return;
          if (current) void policy.refetch(); else setRequested({ object_type: host.object_type, object_id: host.object_id });
        }}>{policy.isFetching ? t("loading") : t("dataflows.fetchPolicy")}</Button>
        <Button variant="outline" disabled={!data?.apply_ready || policy.isFetching} onClick={exportPolicy}>{t("dataflows.exportPolicy")}</Button>
      </div>
    </div>
    {current && policy.isError && <p role="alert" className="text-destructive">{t("dataflows.policyFailed")}</p>}
    {data && <>
      {data.apply_ready ? <output className="text-sm">{t("dataflows.policyReady", { host: data.host.label ?? t("dataflows.unresolved") })}</output>
        : <p role="alert" className="text-sm text-destructive">{t("dataflows.policyBlocked", { host: data.host.label ?? t("dataflows.unresolved") })}</p>}
      <DataflowDiagnostics items={data.diagnostics} policy />
      {data.apply_ready && <DataTable<Rule> className="max-h-96 shrink-0" data={data.rules} getRowKey={rule => JSON.stringify(rule)} columns={[
        { key: "direction", label: t("dataflows.direction"), render: rule => t(`dataflows.directions.${rule.direction}`) },
        { key: "family", label: t("dataflows.family") },
        { key: "source", label: t("dataflows.source") },
        { key: "destination", label: t("dataflows.destination") },
        { key: "transport", label: t("dataflows.transport"), render: rule => rule.transport === "any" ? t("dataflows.anyTransport") : rule.transport.toUpperCase() },
        { key: "destination_ports", label: t("dataflows.ports"), render: rule => rule.destination_ports?.join(", ") ?? t("dataflows.anyPorts") },
        { key: "vrf_id", label: "VRF", render: rule => rule.vrf_id ?? t("dataflows.defaultVrf") },
        { key: "dataflow_ids", label: t("dataflows.provenance"), render: rule => <span className="break-all">{rule.dataflow_ids.join(", ")}</span> },
      ]} emptyState={<p className="p-4 text-sm">{t("dataflows.emptyPolicy")}</p>} />}
    </>}
  </section>;
}
