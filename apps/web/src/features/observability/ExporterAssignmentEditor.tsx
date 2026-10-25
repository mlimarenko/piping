import { useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useLocation, useNavigate, useParams } from "react-router";
import { RequireCapability } from "@/shared/auth/permissions";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { useApiMutation } from "@/shared/api/hooks";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import FormSection from "@/shared/ui-shared/FormSection";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { Switch } from "@/shared/ui-primitives/switch";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import PageState from "@/shared/ui-shared/PageState";
import {
  createExporterAssignment,
  listAllExporterAssignments,
  listExporters,
  updateExporterAssignment,
  type ExporterAssignmentItem,
  type ExporterAssignmentRequest,
  type ExporterCatalogItem,
} from "@/features/observability/api";
import ScopeFields from "./ScopeFields";
import { type MonitoringScopeType } from "./scope";
import { useScopeEntities, type ScopeEntities } from "./useScopeEntities";
import { EXPORTER_ASSIGNMENTS_KEY } from "./keys";
import { EXPORTER_ASSIGNMENTS_BASE, exporterAssignmentsHref, exporterListContext, observabilityRecordHref } from "./observabilityRoutes";

interface DraftState {
  exporter: string;
  name: string;
  scopeType: MonitoringScopeType;
  scopeId: string;
  weight: string;
  enabled: boolean;
  target: string;
  job: string;
  scheme: string;
  port: string;
  metricsPath: string;
  labels: string;
}

const EMPTY_DRAFT: DraftState = {
  exporter: "",
  name: "",
  scopeType: "global",
  scopeId: "",
  weight: "0",
  enabled: true,
  target: "",
  job: "",
  scheme: "",
  port: "",
  metricsPath: "",
  labels: "",
};

function draftFromItem(item: ExporterAssignmentItem): DraftState {
  const labels = item.labels;
  return {
    exporter: item.exporter_slug,
    name: item.name ?? "",
    scopeType: item.scope_type as MonitoringScopeType,
    scopeId: item.scope_id ?? "",
    weight: String(item.weight),
    enabled: item.enabled,
    target: item.target ?? "",
    job: item.job ?? "",
    scheme: item.scheme ?? "",
    port: item.port != null ? String(item.port) : "",
    metricsPath: item.metrics_path ?? "",
    labels:
      labels && typeof labels === "object" ? JSON.stringify(labels, null, 2) : "",
  };
}

function optionalStr(value: string): string | undefined {
  const trimmed = value.trim();
  return trimmed ? trimmed : undefined;
}

function findExporter(exporters: ExporterCatalogItem[], value: string): ExporterCatalogItem | undefined {
  const normalized = value.trim().toLocaleLowerCase();
  if (!normalized) return undefined;
  return exporters.find(
    (exporter) =>
      exporter.slug.toLocaleLowerCase() === normalized ||
      exporter.name.toLocaleLowerCase() === normalized,
  );
}

interface ExporterAssignmentFormProps {
  editing: ExporterAssignmentItem | null;
  exporters: ExporterCatalogItem[];
  entities: ScopeEntities;
  refreshError?: ReactNode;
}

export const ExporterAssignmentForm = ({
  editing,
  exporters,
  entities,
  refreshError,
}: ExporterAssignmentFormProps) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const navigate = useNavigate();
  const { search } = useLocation();
  const { can } = usePermissions();
  const allowed = can(editing ? "UPDATE" : "CREATE", "MONITORING");
  const [baseline] = useState(() => editing ? draftFromItem(editing) : EMPTY_DRAFT);
  const [draft, setDraft] = useState<DraftState>(baseline);
  const [labelsError, setLabelsError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState(false);
  const recordHref = (id: string) => observabilityRecordHref(EXPORTER_ASSIGNMENTS_BASE, id) + exporterListContext(search);
  const back = () => { if (!mutation.isPending) void navigate(editing ? recordHref(editing.id) : exporterAssignmentsHref(search)); };

  const set = <K extends keyof DraftState>(key: K, value: DraftState[K]) =>
    { if (allowed && !mutation.isPending) setDraft((prev) => ({ ...prev, [key]: value })); };

  const selectedExporter = useMemo(
    () => findExporter(exporters, draft.exporter),
    [exporters, draft.exporter],
  );

  // Options for the exporter picker: the catalog profiles, plus the currently
  // selected slug if it isn't in the catalog (so editing a legacy/custom
  // assignment still shows its value rather than rendering blank).
  const exporterOptions = useMemo(() => {
    const opts = exporters.map((e) => ({ slug: e.slug, name: e.name }));
    if (draft.exporter && !opts.some((o) => o.slug === draft.exporter)) {
      opts.push({ slug: draft.exporter, name: draft.exporter });
    }
    return opts;
  }, [exporters, draft.exporter]);

  const buildBody = (): ExporterAssignmentRequest | null => {
    let labels: Record<string, unknown> | undefined;
    const labelsRaw = draft.labels.trim();
    if (labelsRaw) {
      try {
        const parsed: unknown = JSON.parse(labelsRaw);
        if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
          throw new Error("not an object");
        }
        labels = parsed as Record<string, unknown>;
      } catch {
        setLabelsError(
          t("observability.sd.labelsInvalid", { defaultValue: "Labels must be a JSON object." }),
        );
        return null;
      }
    }
    const port = draft.port.trim() ? Number(draft.port) : undefined;
    const exporter = optionalStr(draft.exporter);
    if (!exporter) return null;

    return {
      ...(selectedExporter
        ? { exporter_id: selectedExporter.id }
        : { exporter_slug: exporter, exporter_name: exporter }),
      name: optionalStr(draft.name) ?? null,
      scope_type: draft.scopeType,
      scope_id: draft.scopeType === "global" ? null : draft.scopeId || null,
      weight: Number(draft.weight),
      enabled: draft.enabled,
      target: optionalStr(draft.target) ?? null,
      job: optionalStr(draft.job) ?? null,
      scheme: draft.scheme ? draft.scheme : null,
      port: port != null && Number.isFinite(port) ? port : null,
      metrics_path: optionalStr(draft.metricsPath) ?? null,
      labels: labels ?? null,
    };
  };

  const mutation = useApiMutation({
    mutationFn: (body: ExporterAssignmentRequest) => {
      if (!allowed) throw new Error("Permission removed");
      return editing ? updateExporterAssignment(editing.id, body) : createExporterAssignment(body);
    },
    onSuccess: (saved) => {
      qc.setQueryData<ExporterAssignmentItem[]>(EXPORTER_ASSIGNMENTS_KEY, current => {
        const rows = current ?? [];
        return rows.some(row => row.id === saved.id) ? rows.map(row => row.id === saved.id ? saved : row) : [...rows, saved];
      });
      void qc.invalidateQueries({ queryKey: EXPORTER_ASSIGNMENTS_KEY });
      void qc.invalidateQueries({ queryKey: ["observability", "service-discovery-preview"] });
      notify(
        "success",
        editing
          ? t("observability.sd.updated", { defaultValue: "Assignment updated" })
          : t("observability.sd.created", { defaultValue: "Assignment created" }),
      );
    },
    onError: () => setSaveError(true),
  });

  const submit = () => {
    if (!canSubmit) return;
    setSaveError(false);
    setLabelsError(null);
    const body = buildBody();
    if (!body) return;
    mutation.mutate(body, { onSuccess: saved => { void navigate(recordHref(saved.id)); } });
  };

  const canSubmit =
    allowed && !!draft.exporter.trim() && (draft.scopeType === "global" || !!draft.scopeId) && !mutation.isPending &&
    Number.isInteger(Number(draft.weight)) && (!draft.port.trim() || (Number.isInteger(Number(draft.port)) && Number(draft.port) >= 1 && Number(draft.port) <= 65535));

  return (
    <EntityEditorPage scope="routed"
      onBack={back} backLabel={t("back")} onCancel={back} cancelLabel={t("cancel")}
      onSave={submit} saveLabel={t(editing ? "save" : "create")} saveDisabled={!canSubmit} saving={mutation.isPending}
      error={saveError || refreshError ? <>{saveError && t("observability.workflow.saveFailed", { defaultValue: "Could not save. Your draft is retained. Try again." })}{refreshError}</> : undefined}
      dirty={JSON.stringify(draft) !== JSON.stringify(baseline)} dirtyMessage={t("editor.unsavedChanges")}
      title={
        editing
          ? t("observability.sd.editTitle", { defaultValue: "Edit exporter assignment" })
          : t("observability.sd.createTitle", { defaultValue: "New exporter assignment" })
      }
      description={t("observability.sd.dialogDescription", {
        defaultValue: "Bind an exporter to a scope and override its scrape settings.",
      })}
    >
        <fieldset disabled={mutation.isPending || !allowed} className="grid min-w-0 gap-4">
          <div className="grid gap-1.5">
            <FormSelect
              id="exporter-assignment-exporter"
              label={t("observability.sd.exporter", { defaultValue: "Exporter" })}
              required
              help={t("observability.sd.exporterHint", {
                defaultValue:
                  "The exporter profile sets the default Prometheus job label and groups scrape targets. Pick a different profile (e.g. nginx) to add a separate scrape job — reusing the same profile overrides it instead.",
              })}
              disabled={mutation.isPending}
              value={draft.exporter}
              onValueChange={(v) => set("exporter", v)}
              options={exporterOptions.map((option) => ({
                value: option.slug,
                label: (
                  <>
                    {option.name}
                    <span className="ml-1 font-mono text-xs text-muted-foreground">
                      ({option.slug})
                    </span>
                  </>
                ),
              }))}
              placeholder={t("observability.sd.exporterPlaceholder", {
                defaultValue: "Select an exporter profile…",
              })}
              className="space-y-1"
            />
            {selectedExporter?.description && (
              <p className="text-xs text-muted-foreground">{selectedExporter.description}</p>
            )}
          </div>

          <ScopeFields
            disabled={mutation.isPending}
            scopeType={draft.scopeType}
            scopeId={draft.scopeId}
            onScopeTypeChange={(v) => set("scopeType", v)}
            onScopeIdChange={(v) => set("scopeId", v)}
            entities={entities}
          />

          <div className="grid gap-4 sm:grid-cols-2">
            <FormField
              label={t("observability.sd.name", { defaultValue: "Name (optional)" })}
              htmlFor="exporter-assignment-name"
              className="grid gap-1.5"
            >
              <Input
                className="text-sm"
                value={draft.name}
                id="exporter-assignment-name"
                onChange={(e) => set("name", e.target.value)}
                placeholder={selectedExporter?.name}
              />
            </FormField>
            <FormField
              label={t("observability.sd.weight", { defaultValue: "Weight" })}
              htmlFor="exporter-assignment-weight"
              className="grid gap-1.5"
            >
              <Input
                type="number"
                className="text-sm text-center font-mono"
                value={draft.weight}
                id="exporter-assignment-weight"
                onChange={(e) => set("weight", e.target.value)}
              />
            </FormField>
          </div>

          <FormSection title={t("observability.sd.overrides", { defaultValue: "Overrides (optional)" })} contentClassName="grid gap-4 sm:grid-cols-2">
              <FormField
                label={t("observability.sd.job", { defaultValue: "Job" })}
                htmlFor="exporter-assignment-job"
                className="grid gap-1.5"
              >
                <Input
                  className="text-sm"
                  value={draft.job}
                  id="exporter-assignment-job"
                  onChange={(e) => set("job", e.target.value)}
                  placeholder={selectedExporter?.default_job}
                />
              </FormField>
              <FormField
                label={t("observability.sd.target", { defaultValue: "Target" })}
                htmlFor="exporter-assignment-target"
                className="grid gap-1.5"
              >
                <Input
                  className="text-sm"
                  value={draft.target}
                  id="exporter-assignment-target"
                  onChange={(e) => set("target", e.target.value)}
                  placeholder={selectedExporter?.default_target_source}
                />
              </FormField>
              <FormSelect
                disabled={mutation.isPending}
                label={t("observability.sd.scheme", { defaultValue: "Scheme" })}
                value={draft.scheme || "__default"}
                onValueChange={(v) => set("scheme", v === "__default" ? "" : v)}
                options={[
                  {
                    value: "__default",
                    label: selectedExporter?.default_scheme
                      ? t("observability.sd.schemeDefaultWithValue", {
                          defaultValue: "Default ({{scheme}})",
                          scheme: selectedExporter.default_scheme,
                        })
                      : t("observability.sd.schemeDefault", { defaultValue: "Default" }),
                  },
                  { value: "http", label: "http" },
                  { value: "https", label: "https" },
                ]}
                className="grid gap-1.5"
              />
              <FormField
                label={t("observability.sd.port", { defaultValue: "Port" })}
                htmlFor="exporter-assignment-port"
                className="grid gap-1.5"
              >
                <Input
                  type="number"
                  className="text-sm"
                  value={draft.port}
                  id="exporter-assignment-port"
                  onChange={(e) => set("port", e.target.value)}
                  placeholder={
                    selectedExporter?.default_port != null
                      ? String(selectedExporter.default_port)
                      : undefined
                  }
                  min={1}
                  max={65535}
                />
              </FormField>
              <FormField
                label={t("observability.sd.metricsPath", { defaultValue: "Metrics path" })}
                htmlFor="exporter-assignment-path"
                className="sm:col-span-2"
              >
                <Input
                  className="text-sm"
                  value={draft.metricsPath}
                  id="exporter-assignment-path"
                  onChange={(e) => set("metricsPath", e.target.value)}
                  placeholder={selectedExporter?.default_metrics_path}
                />
              </FormField>
              <FormField
                label={t("observability.sd.labels", { defaultValue: "Labels (JSON object)" })}
                htmlFor="exporter-assignment-labels"
                error={labelsError}
                className="sm:col-span-2"
              >
                <FormFieldControl><textarea
                  id="exporter-assignment-labels"
                  className="min-h-[64px] rounded-md border border-input bg-background px-2 py-1.5 font-mono text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  value={draft.labels}
                  onChange={(e) => set("labels", e.target.value)}
                  placeholder={'{ "env": "prod" }'}
                  spellCheck={false}
                /></FormFieldControl>
              </FormField>
          </FormSection>

          <label className="flex items-center justify-between gap-4 py-2">
            <span className="text-sm font-medium">
              {t("observability.sd.enabled", { defaultValue: "Enabled" })}
            </span>
            <Switch checked={draft.enabled} onCheckedChange={(v) => set("enabled", v)} />
          </label>
      </fieldset>
    </EntityEditorPage>
  );
};

function ExporterAssignmentEditorContent() {
  const { id } = useParams();
  const { t } = useTranslation();
  const { search } = useLocation();
  const navigate = useNavigate();
  const assignments = useQuery({ queryKey: EXPORTER_ASSIGNMENTS_KEY, queryFn: () => listAllExporterAssignments(), enabled: !!id });
  const exporters = useQuery({ queryKey: ["observability", "exporters"], queryFn: () => listExporters(), staleTime: 60_000 });
  const item = assignments.data?.find(row => row.id === id);
  const entities = useScopeEntities(t("observability.workflow.allTargets", { defaultValue: "All matching targets" }), { includeConnectionTargets: item?.scope_type === "connection_target" });
  const pending = (id && assignments.isPending) || exporters.isPending;
  const failed = (id && assignments.isError && !assignments.data) || (exporters.isError && !exporters.data);
  if (pending || failed || (id && !item)) return <EntityEditorPage scope="routed" title={t(id ? "observability.sd.editTitle" : "observability.sd.createTitle")} onBack={() => { void navigate(exporterAssignmentsHref(search)); }} backLabel={t("back")} cancelLabel={t("cancel")}>
    <PageState kind={pending ? "loading" : failed ? "error" : "empty"} title={t(pending ? "loading" : failed ? "observability.entityLoadFailed" : "observability.entityNotFound")} action={failed ? <Button variant="outline" onClick={() => { void assignments.refetch(); void exporters.refetch(); }}>{t("retry")}</Button> : undefined} />
  </EntityEditorPage>;
  return <ExporterAssignmentForm key={id ?? "new"} editing={item ?? null} exporters={exporters.data?.items ?? []} entities={entities}
    refreshError={(id && assignments.isError) || exporters.isError ? <span className="flex flex-wrap items-center gap-2">{t("observability.entityLoadFailed")}<Button variant="outline" onClick={() => { void assignments.refetch(); void exporters.refetch(); }}>{t("retry")}</Button></span> : undefined} />;
}

export default function ExporterAssignmentEditor() {
  const { id } = useParams();
  return <RequireCapability check={p => p.can(id ? "UPDATE" : "CREATE", "MONITORING")}><ExporterAssignmentEditorContent /></RequireCapability>;
}
