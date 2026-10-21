import { useEffect, useRef, useState } from "react";
import { useNavigate, useParams } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Label } from "@/shared/ui-primitives/label";
import { Checkbox } from "@/shared/ui-primitives/checkbox";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/shared/ui-primitives/select";
import { Button } from "@/shared/ui-primitives/button";
import FormSection from "@/shared/ui-shared/FormSection";
import FormField from "@/shared/ui-shared/FormField";
import DataflowEndpointPicker from "./DataflowEndpointPicker";
import { createDataflow, getDataflow, updateDataflow } from "./dataflows-api";
import type { Dataflow, DataflowCreate, DataflowEndpoint } from "./dataflows-api";
import { parseDataflowPorts } from "./dataflowPorts";

const EMPTY: DataflowCreate = {
  name: "", description: "", status: "draft", transport: "any", destination_ports: null, allow_cross_vrf: false,
  source: { object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] },
  destination: { object_type: "dcim.device", object_id: null, address_mode: "default", ipaddresses: [] },
};

function inputEndpoint(endpoint: Dataflow["source"]): DataflowEndpoint {
  return { object_type: endpoint.object_type, object_id: endpoint.object_id, address_mode: endpoint.address_mode, ipaddresses: endpoint.ipaddresses };
}

function endpointValidationKey(endpoint: DataflowEndpoint): string | undefined {
  if (endpoint.address_mode === "selected") {
    if (!endpoint.ipaddresses?.length) return "dataflows.addressError";
    if (endpoint.ipaddresses.length > 32) return "dataflows.addressLimitError";
  }
  if (!["all", "internet"].includes(endpoint.object_type) && !endpoint.object_id) return "dataflows.endpointError";
  return undefined;
}

function DataflowForm({ flow }: Readonly<{ flow?: Dataflow }>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { can } = usePermissions();
  const allowed = can(flow ? "UPDATE" : "CREATE", "INVENTORY");
  const initial: DataflowCreate = flow ? {
    name: flow.name, description: flow.description, status: flow.status, transport: flow.transport,
    destination_ports: flow.destination_ports, allow_cross_vrf: flow.allow_cross_vrf,
    source: inputEndpoint(flow.source), destination: inputEndpoint(flow.destination),
  } : EMPTY;
  const [draft, setDraft] = useState(initial);
  const [portsMode, setPortsMode] = useState(initial.destination_ports ? "selected" : "any");
  const [ports, setPorts] = useState(initial.destination_ports?.join(", ") ?? "");
  const [errors, setErrors] = useState<Record<string, string>>({});
  const formRef = useRef<HTMLFormElement>(null);
  useEffect(() => {
    if (Object.keys(errors).length) {
      const invalidDescription = formRef.current?.querySelector<HTMLTextAreaElement>('#flow-description[aria-invalid="true"]');
      if (invalidDescription) invalidDescription.closest("details")!.open = true;
      formRef.current?.querySelector<HTMLElement>('[aria-invalid="true"], [data-endpoint-invalid="true"] [role="combobox"]')?.focus();
    }
  }, [errors]);
  const mutation = useApiMutation({
    mutationFn: (body: DataflowCreate) => flow ? updateDataflow(flow.id, body) : createDataflow(body),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: ["inventory", "dataflows"] });
      qc.removeQueries({ queryKey: ["inventory", "dataflow-policy"] });
      await navigate("/inventory/dataflows");
    },
  });
  const save = () => {
    if (!allowed || mutation.isPending) return;
    const destination_ports = portsMode === "any" ? null : parseDataflowPorts(ports);
    const next: Record<string, string> = {};
    if (new TextEncoder().encode(draft.name?.trim() ?? "").length > 200) next.name = t("dataflows.nameError");
    if (new TextEncoder().encode(draft.description ?? "").length > 4000) next.description = t("dataflows.descriptionError");
    for (const [key, endpoint] of [["source", draft.source], ["destination", draft.destination]] as const) {
      const errorKey = endpointValidationKey(endpoint);
      if (errorKey) next[key] = t(errorKey);
    }
    if (portsMode === "selected" && !destination_ports) next.ports = t("dataflows.portsError");
    setErrors(next);
    if (Object.keys(next).length) return;
    mutation.mutate({ ...draft, name: draft.name?.trim() || undefined, destination_ports });
  };
  const dirty = JSON.stringify(draft) !== JSON.stringify(initial) || ports !== (initial.destination_ports?.join(", ") ?? "") || portsMode !== (initial.destination_ports ? "selected" : "any");
  return <EntityEditorPage scope="routed" title={flow ? t("dataflows.editTitle", { name: flow.name }) : t("dataflows.newTitle")}
    description={t("dataflows.editorHint")} onBack={() => void navigate("/inventory/dataflows")} backLabel={t("dataflows.back")}
    dirty={dirty && !mutation.isSuccess} dirtyMessage={t("editor.unsavedChanges")} onSave={allowed ? save : undefined}
    saveLabel={t(flow ? "saveChanges" : "create")} cancelLabel={t("cancel")} saving={mutation.isPending}
    error={mutation.isError ? <p role="alert" className="text-destructive">{t("dataflows.saveFailed")}</p> : undefined}>
    <form ref={formRef} className="flex min-w-0 flex-col gap-6" onSubmit={event => { event.preventDefault(); save(); }} noValidate>
      <FormSection title={t("dataflows.endpointsSection")} contentClassName="grid min-w-0 gap-4 @2xl:grid-cols-2">
        <div data-endpoint-invalid={!!errors.source}>
          <DataflowEndpointPicker label={t("dataflows.source")} value={draft.source} onChange={source => setDraft({ ...draft, source })} disabled={!allowed || mutation.isPending} />
        </div>
        <div data-endpoint-invalid={!!errors.destination}>
          <DataflowEndpointPicker label={t("dataflows.destination")} value={draft.destination} onChange={destination => setDraft({ ...draft, destination })} disabled={!allowed || mutation.isPending} />
        </div>
        {(errors.source || errors.destination) && <p role="alert" className="@2xl:col-span-2 text-xs text-destructive">{[...new Set([errors.source, errors.destination].filter(Boolean))].join(" ")}</p>}
        <details className="@2xl:col-span-2 text-xs text-muted-foreground"><summary>{t("dataflows.addressHelp")}</summary><p className="pt-2">{t("dataflows.addressHint")}</p></details>
      </FormSection>
      <FormSection title={t("dataflows.trafficSection")} divider contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
      <FormField label={t("dataflows.transport")}>
        <Select value={draft.transport ?? "any"} onValueChange={transport => setDraft({ ...draft, transport: transport as DataflowCreate["transport"] })} disabled={!allowed || mutation.isPending}>
          <SelectTrigger aria-label={t("dataflows.transport")}><SelectValue /></SelectTrigger>
          <SelectContent>{["any", "tcp", "udp", "sctp"].map(value => <SelectItem key={value} value={value}>{value === "any" ? t("dataflows.anyTransport") : value.toUpperCase()}</SelectItem>)}</SelectContent>
        </Select>
      </FormField>
      <FormField label={t("dataflows.ports")}>
        <Select value={portsMode} onValueChange={mode => { setPortsMode(mode); setPorts(""); }} disabled={!allowed || mutation.isPending}>
          <SelectTrigger aria-label={t("dataflows.ports")}><SelectValue /></SelectTrigger>
          <SelectContent><SelectItem value="any">{t("dataflows.anyPorts")}</SelectItem><SelectItem value="selected">{t("dataflows.specificPorts")}</SelectItem></SelectContent>
        </Select>
        {portsMode === "selected" && <><Input aria-label={t("dataflows.specificPorts")} value={ports} placeholder="443, 8443" disabled={!allowed || mutation.isPending} aria-invalid={!!errors.ports}
          aria-describedby="flow-ports-hint flow-ports-error" onChange={event => setPorts(event.target.value)} />
          <p id="flow-ports-hint" className="text-xs text-muted-foreground">{t("dataflows.portsHint")}</p>
          <p id="flow-ports-error" role={errors.ports ? "alert" : undefined} className="text-xs text-destructive">{errors.ports}</p></>}
      </FormField>
      </FormSection>
      <FormSection title={t("dataflows.metadataSection")} divider contentClassName="grid min-w-0 gap-4 @lg:grid-cols-2">
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
  const { t } = useTranslation();
  const navigate = useNavigate();
  const flow = useQuery({ queryKey: ["inventory", "dataflows", id], queryFn: () => getDataflow(id!), enabled: !!id });
  if (!id) return <DataflowForm />;
  if (flow.isPending || flow.isError || !flow.data) return (
    <EntityEditorPage scope="routed" title={t("dataflows.editTitle", { name: "" })}
      description={t("dataflows.editorHint")} onBack={() => void navigate("/inventory/dataflows")}
      backLabel={t("dataflows.back")} cancelLabel={t("cancel")}>
      {flow.isPending ? <output>{t("loading")}</output> : (
        <div className="flex flex-col items-start gap-4"><p role="alert">{t("dataflows.loadFailed")}</p><Button variant="outline" onClick={() => void flow.refetch()}>{t("retry")}</Button></div>
      )}
    </EntityEditorPage>
  );
  return <DataflowForm key={flow.data.id} flow={flow.data} />;
}
