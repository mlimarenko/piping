import { useQuery } from "@tanstack/react-query";
import { useId } from "react";
import { useTranslation } from "react-i18next";
import EntityPicker from "@/shared/forms/EntityPicker";
import MultiSelect from "@/shared/ui-shared/MultiSelect";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/shared/ui-primitives/select";
import { inventoryResourceOptions, inventoryResourceOptionsQuery } from "./resourceOptions";
import { listAddressGroups } from "./dataflowCatalog-api";
import type { DataflowEndpoint, DataflowEndpointResolution } from "./dataflows-api";
import {
  DATAFLOW_ENDPOINTS,
  DATAFLOW_ENDPOINT_TYPES,
  dataflowAddressModesFor,
  dataflowEndpointKey,
} from "./dataflowEndpoints";
import { DataflowMemberList } from "./DataflowEndpointMembers";
import { useDataflowEndpointResolution } from "./useDataflowEndpointResolution";
import { Button } from "@/shared/ui-primitives/button";

const TYPE_FALLBACK: Record<string, string> = {
  "ipam.iprange": "IP range",
  "extras.tag": "Tag",
  "inventory.addressgroup": "Address group",
};

const ADDRESSABLE_KINDS = ["host", "service", "group", "tag"];

function endpointHint(value: DataflowEndpoint): string {
  if (value.object_type === "all") return "dataflows.allHint";
  if (value.object_type === "internet") return "dataflows.internetHint";
  if (value.object_type === "inventory.addressgroup") return "dataflows.addressGroupHint";
  if (value.object_type === "extras.tag") return "inventory.resources.hints.entities.Tags";
  return `inventory.resources.hints.entities.${DATAFLOW_ENDPOINTS[value.object_type].entity}`;
}

function endpointTypeLabel(t: ReturnType<typeof useTranslation>["t"], type: DataflowEndpoint["object_type"]): string {
  const entity = DATAFLOW_ENDPOINTS[type].entity;
  return t(`dataflows.endpointTypes.${type.replaceAll(".", "_")}`, {
    defaultValue: TYPE_FALLBACK[type] ?? entity ?? type,
  });
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
  return <DataflowMemberList key={dataflowEndpointKey(value)} data={resolution.data} label={label} />;
}

export default function DataflowEndpointPicker({ label, value, onChange, hostsOnly = false, disabled = false, allowedTypes }: Readonly<{
  label: string; value: DataflowEndpoint; onChange: (value: DataflowEndpoint) => void; hostsOnly?: boolean; disabled?: boolean; allowedTypes?: readonly DataflowEndpoint["object_type"][];
}>) {
  const { t } = useTranslation();
  const descriptionId = useId();
  const metadata = DATAFLOW_ENDPOINTS[value.object_type];
  const entity = metadata.entity;
  const builtin = value.object_type === "all" || value.object_type === "internet";
  const isTag = value.object_type === "extras.tag";
  const isAddressGroup = value.object_type === "inventory.addressgroup";
  const supportsAddresses = !hostsOnly && ADDRESSABLE_KINDS.includes(metadata.kind);
  const addressModes = dataflowAddressModesFor(value);
  const options = useQuery({
    ...inventoryResourceOptionsQuery(entity),
    enabled: !!entity && !isAddressGroup,
  });
  const addressGroups = useQuery({
    queryKey: ["inventory", "address-groups", "options"],
    queryFn: () => listAddressGroups(),
    enabled: isAddressGroup,
    staleTime: 30_000,
  });
  const addresses = useDataflowEndpointResolution(value, !hostsOnly && (!isTag || !!value.tag_object_type));
  // Eligible choices use the default selector; selected-mode preview uses the actual intent.
  const choices = useDataflowEndpointResolution(
    { ...value, address_mode: "default", ipaddresses: [] },
    !hostsOnly && supportsAddresses && value.address_mode === "selected",
  );
  const types = DATAFLOW_ENDPOINT_TYPES.filter(type => (!hostsOnly || DATAFLOW_ENDPOINTS[type].kind === "host") && (!allowedTypes || allowedTypes.includes(type)));
  const addressData = addresses.data;
  return <fieldset className="flex min-w-0 flex-col gap-2" disabled={disabled}>
    <legend className="mb-2 text-sm font-medium">{label}</legend>
    <Select value={value.object_type} onValueChange={object_type => onChange({ object_type: object_type as DataflowEndpoint["object_type"], object_id: null, address_mode: "default", ipaddresses: [] })} disabled={disabled}>
      <SelectTrigger aria-label={t("dataflows.endpointType", { label })} aria-describedby={descriptionId}><SelectValue /></SelectTrigger>
      <SelectContent>{types.map(type => <SelectItem key={type} value={type}>{endpointTypeLabel(t, type)}</SelectItem>)}</SelectContent>
    </Select>
    <p id={descriptionId} className="text-xs text-muted-foreground">{t(endpointHint(value))}</p>
    {metadata.kind === "group" && <p className="text-xs text-muted-foreground">{t("dataflows.scopeHint")}</p>}
    {isTag && <>
      <Select value={value.tag_object_type ?? ""} onValueChange={scope => onChange({ ...value, tag_object_type: scope as DataflowEndpoint["tag_object_type"], address_mode: "default", ipaddresses: [] })} disabled={disabled}>
        <SelectTrigger aria-label={t("dataflows.tagScope")}><SelectValue placeholder={t("dataflows.tagScope")} /></SelectTrigger>
        <SelectContent>
          <SelectItem value="dcim.device">{t("dataflows.tagScopeDevice")}</SelectItem>
          <SelectItem value="virtualization.virtualmachine">{t("dataflows.tagScopeVm")}</SelectItem>
        </SelectContent>
      </Select>
    </>}
    {isAddressGroup && <>
      <Select value={value.object_id ?? ""} onValueChange={object_id => onChange({ ...value, object_id, address_mode: "default", ipaddresses: [] })}
        disabled={disabled || addressGroups.isPending || addressGroups.isError}>
        <SelectTrigger aria-label={`${label} · ${t("dataflows.addressGroupsTitle")}`}><SelectValue placeholder={t("dataflows.addressGroupsTitle")} /></SelectTrigger>
        <SelectContent>{(addressGroups.data ?? []).map(group => <SelectItem key={group.id} value={group.id}>{group.name}</SelectItem>)}</SelectContent>
      </Select>
      {addressGroups.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.optionsFailed")}</p>}
    </>}
    {!builtin && !isAddressGroup && entity && <EntityPicker entity={entity} label={label} value={value.object_id ?? ""}
      onChange={object_id => onChange({ ...value, object_id, address_mode: "default", ipaddresses: [] })}
      options={inventoryResourceOptions(entity, options.data?.items ?? [])} loading={options.isPending} disabled={disabled || options.isError} />}
    {!builtin && !isAddressGroup && options.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.optionsFailed")}</p>}
    {supportsAddresses && value.object_id && addressModes.length > 1 && <Select value={value.address_mode ?? "default"} onValueChange={address_mode => onChange({ ...value, address_mode: address_mode as DataflowEndpoint["address_mode"], ipaddresses: [] })} disabled={disabled}>
      <SelectTrigger aria-label={t("dataflows.addressMode", { label })}><SelectValue /></SelectTrigger>
      <SelectContent>{addressModes.map(mode => <SelectItem key={mode} value={mode}>{mode === "oob" ? t("dataflows.addressModeOob") : t(`dataflows.addressModes.${mode}`)}</SelectItem>)}</SelectContent>
    </Select>}
    {supportsAddresses && addressModes.includes("selected") && value.address_mode === "selected" && <>
      <p className="text-xs text-muted-foreground">{t("dataflows.selectedAddressHint")}</p>
      <MultiSelect aria-label={t("dataflows.selectedAddresses", { label })} description={t("inventory.resources.hints.entities.IP Addresses")} items={(choices.data?.ipaddresses ?? []).map(row => ({ id: row.id, label: row.address, searchText: row.vrf_id ?? undefined }))}
        value={value.ipaddresses ?? []} onValueChange={ipaddresses => onChange({ ...value, ipaddresses: [...ipaddresses].sort((left, right) => left.localeCompare(right)) })} disabled={disabled || choices.isPending || choices.isError} />
      {choices.isError && <p role="alert" className="text-sm text-destructive">{t("dataflows.optionsFailed")}</p>}
    </>}
    {supportsAddresses && (value.address_mode ?? "default") === "default" && addressData && <DefaultAddressSummary data={addressData} group={metadata.kind === "group" || metadata.kind === "tag"} />}
    {!hostsOnly && (builtin || value.object_id) && (!isTag || !!value.tag_object_type) && <details className="min-w-0 border-t border-border pt-2">
      <summary className="cursor-pointer rounded-sm text-sm font-medium focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">{t("dataflows.membershipPreview")}</summary>
      <div className="flex min-w-0 flex-col gap-2 pt-2"><MembershipPreview value={value} label={label} resolution={addresses} /></div>
    </details>}
  </fieldset>;
}
