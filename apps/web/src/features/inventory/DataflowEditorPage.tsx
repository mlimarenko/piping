import { useEffect, useId, useRef, useState } from "react";
import { useNavigate, useParams, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageState from "@/shared/ui-shared/PageState";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Label } from "@/shared/ui-primitives/label";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/shared/ui-primitives/select";
import { Button } from "@/shared/ui-primitives/button";
import FormSection from "@/shared/ui-shared/FormSection";
import FormField from "@/shared/ui-shared/FormField";
import DataflowEndpointListEditor, { DATAFLOW_SIDE_LIMIT } from "./DataflowEndpointListEditor";
import DataflowCatalogMetadataFields from "./DataflowCatalogMetadataFields";
import type { EntityCustomFieldsHandle } from "./components/custom-fields/EntityCustomFieldsSection";
import { customFieldDataRecord } from "./components/custom-fields/entityCustomFieldDraft";
import { createDataflow, getDataflow, updateDataflow } from "./dataflows-api";
import type { Dataflow, DataflowCreate, DataflowEndpoint, DataflowEndpointResponse, DataflowTransport } from "./dataflows-api";
import { listApplications, listDataflowGroups } from "./dataflowCatalog-api";
import { dataflowEndpointError } from "./dataflowEndpoints";
import { parseDataflowIcmpTypes, parseDataflowPorts } from "./dataflowPorts";

const TRANSPORTS: DataflowTransport[] = ["any", "tcp", "udp", "tcp_udp", "sctp", "icmp", "icmpv6"];
type ValueMode = "any" | "selected";

function defaultEndpoint(): DataflowEndpoint {
  return { object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] };
}

function blankDraft(): DataflowCreate {
  return {
    name: "", description: "", comments: "", status: "draft", transport: "any",
    destination_ports: null, source_ports: null, icmp_types: null, allow_cross_vrf: false,
    application_id: null, group_id: null, owner_id: null, tenant_id: null, tags: [], custom_field_data: {},
    sources: [defaultEndpoint()], destinations: [defaultEndpoint()],
  };
}

function inputEndpoint(endpoint: DataflowEndpointResponse): DataflowEndpoint {
  return {
    object_type: endpoint.object_type,
    object_id: endpoint.object_id,
    address_mode: endpoint.address_mode ?? "default",
    ipaddresses: [...endpoint.ipaddresses],
    ...(endpoint.object_type === "extras.tag" ? { tag_object_type: endpoint.tag_object_type } : {}),
  };
}

function transportLabel(t: ReturnType<typeof useTranslation>["t"], transport: DataflowTransport): string {
  if (transport === "any") return t("dataflows.anyTransport");
  if (transport === "tcp_udp") return t("dataflows.transportTcpUdp");
  if (transport === "icmp") return t("dataflows.transportIcmp");
  if (transport === "icmpv6") return t("dataflows.transportIcmpv6");
  return transport.toUpperCase();
}

function ValueInput({ label, mode, onMode, value, onValue, hint, error, disabled }: Readonly<{
  label: string; mode: ValueMode; onMode: (mode: ValueMode) => void; value: string; onValue: (value: string) => void;
  hint: string; error?: string; disabled: boolean;
}>) {
  const { t } = useTranslation();
  const inputId = useId();
  const hintId = `${inputId}-hint`;
  const errorId = `${inputId}-error`;
  return <FormField label={label}>
    <Select value={mode} onValueChange={next => { onMode(next as ValueMode); onValue(""); }} disabled={disabled}>
      <SelectTrigger aria-label={t("dataflows.valueMode", { label, defaultValue: "{{label}} mode" })}><SelectValue /></SelectTrigger>
      <SelectContent><SelectItem value="any">{t("dataflows.anyPorts")}</SelectItem><SelectItem value="selected">{t("dataflows.specificPorts")}</SelectItem></SelectContent>
    </Select>
    {mode === "selected" && <>
      <Input aria-label={label} inputMode="numeric" value={value} placeholder="443, 8443" disabled={disabled}
        aria-invalid={!!error} aria-describedby={`${hintId} ${errorId}`} onChange={event => onValue(event.target.value)} />
      <p id={hintId} className="text-xs text-muted-foreground">{hint}</p>
      <p id={errorId} role={error ? "alert" : undefined} className="text-xs text-destructive">{error}</p>
    </>}
  </FormField>;
}

function IcmpInput({ mode, onMode, value, onValue, error, disabled }: Readonly<{
  mode: ValueMode; onMode: (mode: ValueMode) => void; value: string; onValue: (value: string) => void; error?: string; disabled: boolean;
}>) {
  const { t } = useTranslation();
  const inputId = useId();
  const hintId = `${inputId}-hint`;
  const errorId = `${inputId}-error`;
  return <FormField label={t("dataflows.icmpTypes")}>
    <Select value={mode} onValueChange={next => { onMode(next as ValueMode); onValue(""); }} disabled={disabled}>
      <SelectTrigger aria-label={t("dataflows.icmpTypesMode", { defaultValue: "ICMP types mode" })}><SelectValue /></SelectTrigger>
      <SelectContent>
        <SelectItem value="any">{t("dataflows.anyIcmpTypes", { defaultValue: "Any ICMP types" })}</SelectItem>
        <SelectItem value="selected">{t("dataflows.specificIcmpTypes", { defaultValue: "Selected ICMP types" })}</SelectItem>
      </SelectContent>
    </Select>
    {mode === "selected" && <>
      <Input aria-label={t("dataflows.icmpTypes")} inputMode="numeric" value={value} placeholder="0, 3, 8" disabled={disabled}
        aria-invalid={!!error} aria-describedby={`${hintId} ${errorId}`} onChange={event => onValue(event.target.value)} />
      <p id={hintId} className="text-xs text-muted-foreground">{t("dataflows.icmpHint")}</p>
      <p id={errorId} role={error ? "alert" : undefined} className="text-xs text-destructive">{error}</p>
    </>}
  </FormField>;
}

function DataflowForm({ flow, cloneFrom }: Readonly<{ flow?: Dataflow; cloneFrom?: Dataflow }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const allowed = can(cloneFrom ? "CREATE" : flow ? "UPDATE" : "CREATE", "INVENTORY");
  const initial: DataflowCreate = cloneFrom ? {
    ...blankDraft(),
    name: t("dataflows.cloneDraftName", { name: cloneFrom.name }),
    description: cloneFrom.description ?? "",
    comments: cloneFrom.comments ?? "",
    status: "draft",
    transport: cloneFrom.transport,
    destination_ports: cloneFrom.destination_ports ?? null,
    source_ports: cloneFrom.source_ports ?? null,
    icmp_types: cloneFrom.icmp_types ?? null,
    allow_cross_vrf: cloneFrom.allow_cross_vrf,
    application_id: cloneFrom.application_id ?? null,
    group_id: cloneFrom.group_id ?? null,
    owner_id: cloneFrom.owner_id ?? null,
    tenant_id: cloneFrom.tenant_id ?? null,
    tags: [...(cloneFrom.tags ?? [])],
    custom_field_data: customFieldDataRecord(cloneFrom.custom_field_data),
    sources: cloneFrom.sources.map(inputEndpoint),
    destinations: cloneFrom.destinations.map(inputEndpoint),
  } : flow ? {
    name: flow.name, description: flow.description ?? "", comments: flow.comments ?? "", status: flow.status, transport: flow.transport,
    destination_ports: flow.destination_ports ?? null, source_ports: flow.source_ports ?? null, icmp_types: flow.icmp_types ?? null,
    allow_cross_vrf: flow.allow_cross_vrf, application_id: flow.application_id ?? null, group_id: flow.group_id ?? null,
    owner_id: flow.owner_id ?? null, tenant_id: flow.tenant_id ?? null, tags: [...(flow.tags ?? [])],
    custom_field_data: customFieldDataRecord(flow.custom_field_data),
    sources: flow.sources.map(inputEndpoint), destinations: flow.destinations.map(inputEndpoint),
  } : blankDraft();
  const initialDestinationMode: ValueMode = initial.destination_ports?.length ? "selected" : "any";
  const initialSourceMode: ValueMode = initial.source_ports?.length ? "selected" : "any";
  const initialIcmpMode: ValueMode = initial.icmp_types?.length ? "selected" : "any";
  const [draft, setDraft] = useState(initial);
  const [destinationMode, setDestinationMode] = useState<ValueMode>(initialDestinationMode);
  const [destinationPorts, setDestinationPorts] = useState(initial.destination_ports?.join(", ") ?? "");
  const [sourceMode, setSourceMode] = useState<ValueMode>(initialSourceMode);
  const [sourcePorts, setSourcePorts] = useState(initial.source_ports?.join(", ") ?? "");
  const [icmpMode, setIcmpMode] = useState<ValueMode>(initialIcmpMode);
  const [icmpTypes, setIcmpTypes] = useState(initial.icmp_types?.join(", ") ?? "");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const formRef = useRef<HTMLFormElement>(null);
  const customFields = useRef<EntityCustomFieldsHandle | null>(null);
  useEffect(() => {
    if (Object.keys(errors).length) {
      const invalidDescription = formRef.current?.querySelector<HTMLTextAreaElement>('#flow-description[aria-invalid="true"]');
      if (invalidDescription) invalidDescription.closest("details")!.open = true;
      formRef.current?.querySelector<HTMLElement>('[aria-invalid="true"], [data-endpoint-invalid="true"][tabindex], [data-endpoint-invalid="true"] [role="combobox"]')?.focus();
    }
  }, [errors]);
  const mutation = useApiMutation({
    mutationFn: (body: DataflowCreate) => flow && !cloneFrom ? updateDataflow(flow.id, body) : createDataflow(body),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["inventory", "dataflows"] });
      qc.removeQueries({ queryKey: ["inventory", "dataflow-policy"] });
      await navigate("/inventory/dataflows");
    },
  });
  const applications = useQuery({ queryKey: ["inventory", "applications", "options"], queryFn: () => listApplications(), staleTime: 60_000 });
  const groups = useQuery({ queryKey: ["inventory", "dataflow-groups", "options"], queryFn: () => listDataflowGroups(), staleTime: 60_000 });
  const portsAllowed = draft.transport !== "icmp" && draft.transport !== "icmpv6";
  const icmpAllowed = !portsAllowed;
  const save = () => {
    if (!allowed || mutation.isPending) return;
    const next: Record<string, string> = {};
    if (new TextEncoder().encode(draft.name?.trim() ?? "").length > 200) next.name = t("dataflows.nameError");
    if (new TextEncoder().encode(draft.description ?? "").length > 4000) next.description = t("dataflows.descriptionError");
    for (const side of ["sources", "destinations"] as const) {
      if (!draft[side].length) next[side] = t("dataflows.endpointRequired");
      else if (draft[side].length > DATAFLOW_SIDE_LIMIT) next[side] = t("dataflows.sideLimit");
      else {
        const endpointError = draft[side].map(dataflowEndpointError).find(Boolean);
        if (endpointError) next[side] = t(endpointError);
      }
    }
    const destination_ports = !portsAllowed ? null : destinationMode === "any" ? null : parseDataflowPorts(destinationPorts);
    const source_ports = !portsAllowed ? null : sourceMode === "any" ? null : parseDataflowPorts(sourcePorts);
    const icmp_types = !icmpAllowed || icmpMode === "any" ? null : parseDataflowIcmpTypes(icmpTypes);
    if (portsAllowed && destinationMode === "selected" && !destination_ports) next.ports = t("dataflows.portsError");
    if (portsAllowed && sourceMode === "selected" && !source_ports) next.sourcePorts = t("dataflows.portsError");
    if (icmpAllowed && icmpMode === "selected" && !icmp_types) next.icmp = t("dataflows.icmpError");
    let custom_field_data = customFieldDataRecord(draft.custom_field_data);
    try {
      custom_field_data = customFields.current?.buildPayload() ?? custom_field_data;
    } catch {
      next.custom = t("customFields.entity.definitionsUnavailable");
    }
    setErrors(next);
    if (Object.keys(next).length) return;
    mutation.mutate({
      ...draft,
      name: draft.name?.trim() || undefined,
      destination_ports,
      source_ports,
      icmp_types,
      sources: draft.sources,
      destinations: draft.destinations,
      custom_field_data,
    });
  };
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial)
    || destinationMode !== initialDestinationMode || destinationPorts !== (initial.destination_ports?.join(", ") ?? "")
    || sourceMode !== initialSourceMode || sourcePorts !== (initial.source_ports?.join(", ") ?? "")
    || icmpMode !== initialIcmpMode || icmpTypes !== (initial.icmp_types?.join(", ") ?? "");
  return <EntityEditorPage scope="routed" title={flow ? t("dataflows.editTitle", { name: flow.name }) : t("dataflows.newTitle")}
    description={t("dataflows.editorHint")} onBack={() => void navigate("/inventory/dataflows")} backLabel={t("dataflows.back")}
    dirty={dirty && !mutation.isSuccess} dirtyMessage={t("editor.unsavedChanges")} onSave={allowed ? save : undefined}
    saveLabel={t(flow ? "saveChanges" : "create")} cancelLabel={t("cancel")} saving={mutation.isPending}
    error={mutation.isError ? <p role="alert" className="text-destructive">{t("dataflows.saveFailed")}</p> : undefined}>
    <form ref={formRef} className="flex min-w-0 flex-col gap-4" onSubmit={event => { event.preventDefault(); save(); }} noValidate>
      <FormSection title={t("dataflows.endpointsSection")} contentClassName="grid min-w-0 gap-6 @2xl:grid-cols-2">
        <div className="min-w-0" data-endpoint-invalid={!!errors.sources}>
          <DataflowEndpointListEditor label={t("dataflows.sources")} rowLabel={t("dataflows.source")} addLabel={t("dataflows.addSource")}
            value={draft.sources} onChange={sources => { setDraft({ ...draft, sources }); setErrors(current => { if (!current.sources) return current; const next = { ...current }; delete next.sources; return next; }); }} disabled={!allowed || mutation.isPending} showErrors={!!errors.sources} />
        </div>
        <div className="min-w-0" data-endpoint-invalid={!!errors.destinations}>
          <DataflowEndpointListEditor label={t("dataflows.destinations")} rowLabel={t("dataflows.destination")} addLabel={t("dataflows.addDestination")}
            value={draft.destinations} onChange={destinations => { setDraft({ ...draft, destinations }); setErrors(current => { if (!current.destinations) return current; const next = { ...current }; delete next.destinations; return next; }); }} disabled={!allowed || mutation.isPending} showErrors={!!errors.destinations} />
        </div>
        <details className="@2xl:col-span-2 text-xs text-muted-foreground"><summary>{t("dataflows.addressHelp")}</summary><p className="pt-2">{t("dataflows.addressHint")}</p></details>
      </FormSection>
      <FormSection title={t("dataflows.trafficSection")} contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
        <FormField label={t("dataflows.transport")}>
          <Select value={draft.transport ?? "any"} onValueChange={transport => setDraft({ ...draft, transport: transport as DataflowTransport })} disabled={!allowed || mutation.isPending}>
            <SelectTrigger aria-label={t("dataflows.transport")}><SelectValue /></SelectTrigger>
            <SelectContent>{TRANSPORTS.map(value => <SelectItem key={value} value={value}>{transportLabel(t, value)}</SelectItem>)}</SelectContent>
          </Select>
        </FormField>
        {portsAllowed && <>
          <ValueInput label={t("dataflows.ports")} mode={destinationMode} onMode={setDestinationMode} value={destinationPorts} onValue={setDestinationPorts}
            hint={t("dataflows.portsHint")} error={errors.ports} disabled={!allowed || mutation.isPending} />
          <ValueInput label={t("dataflows.sourcePorts")} mode={sourceMode} onMode={setSourceMode} value={sourcePorts} onValue={setSourcePorts}
            hint={t("dataflows.sourcePortsHint")} error={errors.sourcePorts} disabled={!allowed || mutation.isPending} />
        </>}
        {icmpAllowed && <>
          <IcmpInput mode={icmpMode} onMode={setIcmpMode} value={icmpTypes} onValue={setIcmpTypes} error={errors.icmp} disabled={!allowed || mutation.isPending} />
          <p className="text-xs text-muted-foreground">{t("dataflows.portsIcmpHint")}</p>
        </>}
      </FormSection>
      <FormSection title={t("dataflows.metadataSection")} contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
        <FormField label={t("dataflows.optionalName")} htmlFor="flow-name">
          <Input id="flow-name" value={draft.name ?? ""} placeholder={t("dataflows.namePlaceholder")} disabled={!allowed || mutation.isPending} aria-invalid={!!errors.name} aria-describedby={errors.name ? "flow-name-error" : undefined}
            onChange={event => setDraft({ ...draft, name: event.target.value })} />
          {errors.name && <p id="flow-name-error" role="alert" className="text-sm text-destructive">{errors.name}</p>}
        </FormField>
        <FormField label={t("status")}>
          <Select value={draft.status ?? "draft"} onValueChange={status => setDraft({ ...draft, status: status as DataflowCreate["status"] })} disabled={!allowed || mutation.isPending}>
            <SelectTrigger aria-label={t("status")}><SelectValue /></SelectTrigger>
            <SelectContent>{["draft", "active", "disabled"].map(value => <SelectItem key={value} value={value}>{t(`dataflows.statuses.${value}`)}</SelectItem>)}</SelectContent>
          </Select>
        </FormField>
        <FormField label={t("dataflows.applicationFilter")}>
          <Select value={draft.application_id ?? "none"} onValueChange={application_id => setDraft({ ...draft, application_id: application_id === "none" ? null : application_id })} disabled={!allowed || mutation.isPending || applications.isError}>
            <SelectTrigger aria-label={t("dataflows.applicationFilter")}><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="none">{t("dataflows.noApplication")}</SelectItem>
              {(applications.data ?? []).map(application => <SelectItem key={application.id} value={application.id}>{application.name}</SelectItem>)}
            </SelectContent>
          </Select>
        </FormField>
        <FormField label={t("dataflows.groupFilter")}>
          <Select value={draft.group_id ?? "none"} onValueChange={group_id => setDraft({ ...draft, group_id: group_id === "none" ? null : group_id })} disabled={!allowed || mutation.isPending || groups.isError}>
            <SelectTrigger aria-label={t("dataflows.groupFilter")}><SelectValue /></SelectTrigger>
            <SelectContent>
              <SelectItem value="none">{t("dataflows.noGroup")}</SelectItem>
              {(groups.data ?? []).map(group => <SelectItem key={group.id} value={group.id}>{group.name}</SelectItem>)}
            </SelectContent>
          </Select>
        </FormField>
        <FormField label={t("inventory.resources.fields.comments")} htmlFor="flow-comments" className="@lg:col-span-2">
          <Textarea id="flow-comments" value={draft.comments ?? ""} disabled={!allowed || mutation.isPending}
            onChange={event => setDraft({ ...draft, comments: event.target.value })} />
        </FormField>
        <div className="min-w-0 @lg:col-span-2">
          <DataflowCatalogMetadataFields
            draft={{ owner_id: draft.owner_id, tenant_id: draft.tenant_id, tags: draft.tags }}
            onChange={change => setDraft({ ...draft, ...change })}
            disabled={!allowed || mutation.isPending}
            entityKind="inventory.dataflow" entityId={flow?.id}
            creationScopeToken={flow ? undefined : "inventory.dataflow"}
            customFields={customFieldDataRecord(draft.custom_field_data)}
            onRegisterCustomFields={handle => { customFields.current = handle; }} />
          {errors.custom && <p role="alert" className="text-sm text-destructive">{errors.custom}</p>}
        </div>
      </FormSection>
      <details className="border-t border-border pt-4"><summary className="text-sm font-semibold">{t("dataflows.advanced")}</summary>
        <div className="flex flex-col gap-4 py-4">
          <div className="flex items-start gap-2"><Checkbox id="flow-cross-vrf" checked={draft.allow_cross_vrf ?? false} disabled={!allowed || mutation.isPending}
            onCheckedChange={checked => setDraft({ ...draft, allow_cross_vrf: checked === true })} /><Label htmlFor="flow-cross-vrf">{t("dataflows.crossVrf")}</Label></div>
          <p className="text-xs text-muted-foreground">{t("dataflows.crossVrfHint")}</p>
          <Label htmlFor="flow-description">{t("description")}</Label>
          <Textarea id="flow-description" value={draft.description ?? ""} disabled={!allowed || mutation.isPending} aria-invalid={!!errors.description}
            aria-describedby={errors.description ? "flow-description-error" : undefined} onChange={event => setDraft({ ...draft, description: event.target.value })} />
          {errors.description && <p id="flow-description-error" role="alert" className="text-sm text-destructive">{errors.description}</p>}
        </div>
      </details>
      <button type="submit" className="sr-only" tabIndex={-1} disabled={!allowed || mutation.isPending}>{t("save")}</button>
    </form>
  </EntityEditorPage>;
}

export default function DataflowEditorPage() {
  const { id } = useParams();
  const [searchParams] = useSearchParams();
  const { t } = useTranslation();
  const navigate = useNavigate();
  const cloneId = id ? null : searchParams.get("clone");
  const loadId = cloneId ?? id;
  const flow = useQuery({ queryKey: ["inventory", "dataflows", loadId], queryFn: () => getDataflow(loadId!), enabled: !!loadId });
  if (!loadId) return <DataflowForm />;
  if (flow.isPending || flow.isError || !flow.data) return (
    <EntityEditorPage scope="routed" title={t("dataflows.editTitle", { name: "" })}
      description={t("dataflows.editorHint")} onBack={() => void navigate("/inventory/dataflows")}
      backLabel={t("dataflows.back")} cancelLabel={t("cancel")}>
      {flow.isPending ? <PageState kind="loading" title={t("loading")} /> : (
        <PageState kind="error" title={t("dataflows.loadFailed")}
          action={<Button variant="outline" onClick={() => void flow.refetch()}>{t("retry")}</Button>} />
      )}
    </EntityEditorPage>
  );
  return cloneId
    ? <DataflowForm key={flow.data.id} cloneFrom={flow.data} />
    : <DataflowForm key={flow.data.id} flow={flow.data} />;
}
