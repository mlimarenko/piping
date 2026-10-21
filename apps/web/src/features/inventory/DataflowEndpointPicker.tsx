import { useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import EntityPicker from "@/shared/forms/EntityPicker";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/shared/ui-primitives/select";
import { listAllResources } from "./api";
import { inventoryResourceOption } from "./resourceOptions";
import type { DataflowEndpoint, DataflowEndpointResolution } from "./dataflows-api";
import { DATAFLOW_ENDPOINTS, DATAFLOW_ENDPOINT_TYPES } from "./dataflowEndpoints";
import { DataflowMemberList } from "./DataflowEndpointMembers";
import { useDataflowEndpointResolution } from "./useDataflowEndpointResolution";
import { Button } from "@/shared/ui-primitives/button";

function endpointHint(value: DataflowEndpoint): string {
  if (value.object_type === "all") return "dataflows.allHint";
  if (value.object_type === "internet") return "dataflows.internetHint";
  return `inventory.resources.hints.entities.${DATAFLOW_ENDPOINTS[value.object_type].entity}`;
}

function DefaultAddressSummary({ data, group }: Readonly<{ data: DataflowEndpointResolution; group: boolean }>) {
  const { t } = useTranslation();
  if (group) return <p className="text-sm text-muted-foreground">{t("dataflows.defaultAddressCount", { count: data.default_ipaddresses.length })}</p>;
  const addresses = data.ipaddresses.filter(row => data.default_ipaddresses.includes(row.id)).map(row => row.address).join(", ");
  return <p className="text-sm text-muted-foreground">{t("dataflows.effectiveDefaults", { addresses: addresses || t("dataflows.noDefaultAddresses") })}</p>;
}

function MembershipPreview({ value, label, resolution }: Readonly<{
  value: DataflowEndpoint; label: string; resolution: ReturnType<typeof useDataflowEndpointResolution>;
}>) {
  const { t } = useTranslation();
  if (value.address_mode === "selected" && !value.ipaddresses?.length) return <p className="text-sm text-muted-foreground">{t("dataflows.addressError")}</p>;
  if (resolution.isPending) return <output className="text-sm text-muted-foreground">{t("dataflows.membersLoading")}</output>;
  if (resolution.isError) return <><p role="alert" className="text-sm text-destructive">{t("dataflows.resolutionFailed")}</p><Button type="button" variant="outline" size="sm" onClick={() => void resolution.refetch()}>{t("retry")}</Button></>;
  return <DataflowMemberList key={`${value.object_type}:${value.object_id}`} data={resolution.data} label={label} />;
}

export default function DataflowEndpointPicker({ label, value, onChange, hostsOnly = false, disabled = false }: Readonly<{
  label: string; value: DataflowEndpoint; onChange: (value: DataflowEndpoint) => void; hostsOnly?: boolean; disabled?: boolean;
}>) {
  const { t } = useTranslation();
  const descriptionId = useId();
  const metadata = DATAFLOW_ENDPOINTS[value.object_type];
  const entity = metadata.entity;
  const builtin = value.object_type === "all" || value.object_type === "internet";
  const addressable = !hostsOnly && ["host", "service", "group"].includes(metadata.kind);
  const options = useQuery({
    queryKey: ["inventory", "resources", entity, "options"],
    queryFn: () => {
      if (!entity) throw new Error("Unsupported endpoint type");
      return listAllResources(entity);
    },
    enabled: !!entity, staleTime: 30_000,
  });
  const addresses = useDataflowEndpointResolution(value, !hostsOnly);
  // Eligible choices use the default selector; selected-mode preview uses the actual intent.
  const choices = useDataflowEndpointResolution({ ...value, address_mode: "default", ipaddresses: [] }, !hostsOnly && addressable && value.address_mode === "selected");
  const types = DATAFLOW_ENDPOINT_TYPES.filter(type => !hostsOnly || DATAFLOW_ENDPOINTS[type].kind === "host");
  const addressData = addresses.data;
  return <fieldset className="flex min-w-0 flex-col gap-2" disabled={disabled}>
    <legend className="mb-2 text-sm font-medium">{label}</legend>
    <Select value={value.object_type} onValueChange={object_type => onChange({ object_type: object_type as DataflowEndpoint["object_type"], object_id: null, address_mode: "default", ipaddresses: [] })} disabled={disabled}>
      <SelectTrigger aria-label={t("dataflows.endpointType", { label })} aria-describedby={descriptionId}><SelectValue /></SelectTrigger>
      <SelectContent>{types.map(type => <SelectItem key={type} value={type}>{t(`dataflows.endpointTypes.${type.replaceAll(".", "_")}`)}</SelectItem>)}</SelectContent>
    </Select>
    <p id={descriptionId} className="text-xs text-muted-foreground">{t(endpointHint(value))}</p>
    {metadata.kind === "group" && <p className="text-xs text-muted-foreground">{t("dataflows.scopeHint")}</p>}
    {!builtin && entity && <EntityPicker entity={entity} label={label} value={value.object_id ?? ""}
      onChange={object_id => onChange({ ...value, object_id, address_mode: "default", ipaddresses: [] })}
      options={(options.data ?? []).map(row => inventoryResourceOption(entity, row))} loading={options.isPending} disabled={disabled || options.isError} />}
    {!builtin && options.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.optionsFailed")}</p>}
    {addressable && (builtin || value.object_id) && <>
      <Select value={value.address_mode ?? "default"} onValueChange={address_mode => onChange({ ...value, address_mode: address_mode as DataflowEndpoint["address_mode"], ipaddresses: [] })} disabled={disabled}>
        <SelectTrigger aria-label={t("dataflows.addressMode", { label })}><SelectValue /></SelectTrigger>
        <SelectContent>{["default", "selected", "all"].map(mode => <SelectItem key={mode} value={mode}>{t(`dataflows.addressModes.${mode}`)}</SelectItem>)}</SelectContent>
      </Select>
      {value.address_mode === "selected" && <>
        <p className="text-xs text-muted-foreground">{t("dataflows.selectedAddressHint")}</p>
        <MultiSelect aria-label={t("dataflows.selectedAddresses", { label })} description={t("inventory.resources.hints.entities.IP Addresses")} items={(choices.data?.ipaddresses ?? []).map(row => ({ id: row.id, label: row.address, searchText: row.vrf_id ?? undefined }))}
          value={value.ipaddresses ?? []} onValueChange={ipaddresses => onChange({ ...value, ipaddresses })} disabled={disabled || choices.isPending || choices.isError} />
        {choices.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.optionsFailed")}</p>}
      </>}
      {(value.address_mode ?? "default") === "default" && addressData && <DefaultAddressSummary data={addressData} group={metadata.kind === "group"} />}
    </>}
    {!hostsOnly && (builtin || value.object_id) && <details className="min-w-0 border-t border-border pt-2">
      <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("dataflows.membershipPreview")}</summary>
      <div className="flex min-w-0 flex-col gap-2 pt-2"><MembershipPreview value={value} label={label} resolution={addresses} /></div>
    </details>}
  </fieldset>;
}
