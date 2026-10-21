import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronLeft, ChevronRight, Rocket } from "lucide-react";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { cn } from "@/shared/lib/utils";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import {
  listMonitoringPresets,
  monitoringQuickStart,
  type ExporterPresetCatalogItem,
} from "@/features/observability/api";
import ScopeFields from "./ScopeFields";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { MONITORING_SOURCE_TYPES } from "./monitoringTaxonomy";
import { type MonitoringScopeType } from "./scope";
import { useScopeEntities } from "./useScopeEntities";
import {
  METRIC_QUERIES_KEY,
  METRIC_SETS_KEY,
  METRIC_SET_ASSIGNMENTS_KEY,
  PRESETS_KEY,
  QUERY_SOURCES_KEY,
  QUERY_SOURCE_ASSIGNMENTS_KEY,
  invalidateInspectorMetricCaches,
} from "./keys";

type WizardStep = 1 | 2 | 3;

interface DraftState {
  // Step 1
  queryUrl: string;
  sourceType: string;
  credentialRef: string;
  name: string;
  // Step 2 — empty string means "skip preset"; we never expose a UUID-shaped
  // sentinel through EntitySelect (it id-binds on the value).
  presetSlug: string;
  // Step 3
  scopeType: MonitoringScopeType;
  scopeId: string;
}

const EMPTY_DRAFT: DraftState = {
  queryUrl: "",
  sourceType: "prometheus",
  credentialRef: "",
  name: "",
  presetSlug: "",
  scopeType: "global",
  scopeId: "",
};

interface QuickStartWizardProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called once the wizard succeeds so the host page can switch views. */
  onCompleted?: () => void;
}

/**
 * Three-step quick-start wizard:
 *   1. Source — URL + source_type radio + optional named-secret UUID
 *   2. Preset — pick one of the built-in exporter presets, or skip
 *   3. Scope — reuse {@link ScopeFields}
 * On submit, calls POST /api/v1/monitoring/quick-start, which atomically
 * creates the query source, attaches it to the scope, and applies the preset
 * if chosen. The whole composite runs in one transaction.
 */
const QuickStartWizard = ({ open, onOpenChange, onCompleted }: QuickStartWizardProps) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const globalLabel = t("observability.scope.global", { defaultValue: "Global" });
  const entities = useScopeEntities(globalLabel);

  const [step, setStep] = useState<WizardStep>(1);
  const [draft, setDraft] = useState<DraftState>(EMPTY_DRAFT);

  // Reset whenever the dialog (re)opens.
  const formKey = open ? "open" : "closed";
  const [prevFormKey, setPrevFormKey] = useState(formKey);
  if (prevFormKey !== formKey) {
    setPrevFormKey(formKey);
    setStep(1);
    setDraft(EMPTY_DRAFT);
  }

  const set = <K extends keyof DraftState>(key: K, value: DraftState[K]) =>
    { if (!submitMutation.isPending) setDraft((prev) => ({ ...prev, [key]: value })); };

  // Catalog of built-in presets — the wizard only lists exporters that
  // actually have a preset, so no Finish-time 404s on "I picked nginx".
  const presetsQuery = useQuery({
    queryKey: PRESETS_KEY,
    queryFn: () => listMonitoringPresets(),
    enabled: open && step >= 2,
  });

  const presets = useMemo<ExporterPresetCatalogItem[]>(
    () => presetsQuery.data?.items ?? [],
    [presetsQuery.data],
  );

  const submitMutation = useApiMutation({
    mutationFn: (input: DraftState) =>
      monitoringQuickStart({
        query_url: input.queryUrl,
        source_type: input.sourceType,
        name: input.name.trim() || null,
        credential_ref: input.credentialRef.trim() || null,
        preset_slug: input.presetSlug ? input.presetSlug : null,
        scope_type: input.scopeType,
        scope_id: input.scopeType === "global" ? null : input.scopeId || null,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: QUERY_SOURCES_KEY });
      void qc.invalidateQueries({ queryKey: QUERY_SOURCE_ASSIGNMENTS_KEY });
      void qc.invalidateQueries({ queryKey: METRIC_QUERIES_KEY });
      void qc.invalidateQueries({ queryKey: METRIC_SETS_KEY });
      void qc.invalidateQueries({ queryKey: METRIC_SET_ASSIGNMENTS_KEY });
      invalidateInspectorMetricCaches(qc);
      notify(
        "success",
        t("observability.quickStart.done", { defaultValue: "Quick start applied" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const step1Valid = draft.queryUrl.trim().length > 0 && !!draft.sourceType;
  const { confirmExit, discardDialog } = useUnsavedChangesGuard(open && JSON.stringify(draft) !== JSON.stringify(EMPTY_DRAFT));
  const close = () => {
    if (submitMutation.isPending) return;
    if (JSON.stringify(draft) !== JSON.stringify(EMPTY_DRAFT)) confirmExit(() => onOpenChange(false));
    else onOpenChange(false);
  };
  const step3Valid = draft.scopeType === "global" || !!draft.scopeId;

  const goNext = () => {
    if (submitMutation.isPending) return;
    if (step === 1 && step1Valid) setStep(2);
    else if (step === 2 && step1Valid) setStep(3);
  };
  const goBack = () => {
    if (submitMutation.isPending) return;
    if (step === 3) setStep(2);
    else if (step === 2) setStep(1);
  };

  const sourceTypeLabel = (type: string) => {
    if (type === "victoriametrics") {
      return t("observability.source.families.victoriametrics.label", {
        defaultValue: "VictoriaMetrics (PromQL-compatible)",
      });
    }
    if (type === "zabbix") {
      return t("observability.source.families.zabbix.label", { defaultValue: "Zabbix" });
    }
    return t("observability.source.families.prometheus.label", {
      defaultValue: "Prometheus-compatible",
    });
  };

  const renderStepper = () => (
    <ol className="mb-4 flex items-center gap-2 text-xs font-medium">
      {[1, 2, 3].map((n) => {
        const labels = [
          t("observability.quickStart.steps.source", { defaultValue: "Source" }),
          t("observability.quickStart.steps.preset", { defaultValue: "Preset" }),
          t("observability.quickStart.steps.scope", { defaultValue: "Scope" }),
        ];
        const isActive = n === step;
        const isDone = n < step;
        return (
          <li key={n} className="flex items-center gap-2">
            <span
              className={cn(
                "inline-flex h-5 w-5 items-center justify-center rounded-full border",
                isDone
                  ? "border-primary bg-primary text-primary-foreground"
                  : isActive
                    ? "border-primary text-primary"
                    : "border-border text-muted-foreground",
              )}
            >
              {isDone ? <Check className="h-3 w-3" /> : n}
            </span>
            <span className={cn(isActive ? "text-foreground" : "text-muted-foreground")}>
              {labels[n - 1]}
            </span>
            {n < 3 && <ChevronRight className="h-3 w-3 text-muted-foreground" />}
          </li>
        );
      })}
    </ol>
  );

  const renderStep1 = () => (
    <div className="grid gap-3">
      <FormField
        label={t("observability.quickStart.queryUrl", { defaultValue: "Query URL" })}
        htmlFor="quick-start-url"
        required
        help={t("observability.quickStart.queryUrlHint", {
          defaultValue:
            "Use a Prometheus-compatible /api/v1/query endpoint (VictoriaMetrics supports this) or a Zabbix JSON-RPC URL.",
        })}
      >
        <FormFieldControl><Input
          id="quick-start-url"
          type="url"
          className="text-sm"
          value={draft.queryUrl}
          onChange={(event) => set("queryUrl", event.target.value)}
          placeholder="http://victoriametrics:8428/api/v1/query"
        /></FormFieldControl>
      </FormField>

      <FormField
        label={t("observability.quickStart.sourceType", { defaultValue: "Source type" })}
        required
      >
        <div className="flex flex-wrap gap-2" role="radiogroup" aria-label={t("observability.quickStart.sourceType", { defaultValue: "Source type" })}>
          {MONITORING_SOURCE_TYPES.map((type) => {
            const selected = draft.sourceType === type;
            return (
              <button
                key={type}
                type="button"
                role="radio"
                aria-checked={selected}
                onClick={() => set("sourceType", type)}
                className={cn(
                  "flex-1 rounded-md border px-3 py-2 text-left text-xs",
                  selected
                    ? "border-primary bg-accent text-foreground"
                    : "border-input/30 bg-background text-muted-foreground hover:border-input/60",
                )}
              >
                <span className="block font-medium text-foreground">
                  {sourceTypeLabel(type)}
                </span>
              </button>
            );
          })}
        </div>
      </FormField>

      <FormField
        label={t("observability.quickStart.credentialRef", {
          defaultValue: "Bearer-token named secret ID (optional)",
        })}
        htmlFor="quick-start-credential"
        help={t("observability.quickStart.credentialRefHint", {
          defaultValue:
            "UUID of an existing named secret used as the bearer token; its value is never stored here.",
        })}
      >
        <FormFieldControl><Input
          className="text-sm font-mono"
          value={draft.credentialRef}
          id="quick-start-credential"
          onChange={(event) => set("credentialRef", event.target.value)}
          placeholder="00000000-0000-0000-0000-000000000000"
        /></FormFieldControl>
      </FormField>

      <FormField
        label={t("observability.quickStart.name", { defaultValue: "Display name (optional)" })}
        htmlFor="quick-start-name"
      >
        <Input
          className="text-sm"
          value={draft.name}
          id="quick-start-name"
          onChange={(event) => set("name", event.target.value)}
        />
      </FormField>
    </div>
  );

  const skipLabel = t("observability.quickStart.skipPreset", {
    defaultValue: "Skip — only create the data source",
  });
  const renderStep2 = () => {
    const items = [
      // The "skip" option binds the empty-string sentinel; the wizard sends
      // `preset_slug: null` whenever this is the active value.
      { id: "", label: skipLabel },
      ...presets.map((preset) => ({
        id: preset.exporter_slug,
        label: preset.description
          ? `${preset.name} — ${preset.description}`
          : preset.name,
      })),
    ];
    return (
      <div className="grid gap-3">
        <FormField
          label={t("observability.preset.exporter", { defaultValue: "Exporter preset" })}
          htmlFor="quick-start-preset"
          help={
            presetsQuery.isPending
              ? t("loading", { defaultValue: "Loading…" })
              : presets.length === 0
                ? t("observability.preset.exporterEmpty", {
                    defaultValue: "No exporter catalog entries available.",
                  })
                : t("observability.quickStart.presetHint", {
                    defaultValue:
                      "Picking a preset adds matching metric queries and a metric set.",
                  })
          }
        >
          <FormFieldControl><EntitySelect
            id="quick-start-preset"
            disabled={submitMutation.isPending}
            value={draft.presetSlug}
            onValueChange={(v) => set("presetSlug", v)}
            triggerClassName="text-sm"
            placeholder={skipLabel}
            searchPlaceholder={t("search")}
            items={items}
          /></FormFieldControl>
        </FormField>
      </div>
    );
  };

  const renderStep3 = () => (
    <div className="grid gap-3">
      <ScopeFields
        disabled={submitMutation.isPending}
        scopeType={draft.scopeType}
        scopeId={draft.scopeId}
        onScopeTypeChange={(v) => set("scopeType", v)}
        onScopeIdChange={(v) => set("scopeId", v)}
        entities={entities}
      />
    </div>
  );

  return (
    <Dialog open={open} onOpenChange={(next) => { if (!next) close(); }}>
      <DialogContent
        className="max-w-xl"
        description={t("observability.quickStart.description", {
          defaultValue: "Set up a monitoring source, optional preset, and scope in one go.",
        })}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Rocket className="h-4 w-4 text-primary" />
            {t("observability.quickStart.title", { defaultValue: "Quick start" })}
          </DialogTitle>
        </DialogHeader>

        {renderStepper()}
        <fieldset disabled={submitMutation.isPending} className="min-w-0">
        {step === 1 && renderStep1()}
        {step === 2 && renderStep2()}
        {step === 3 && renderStep3()}
        </fieldset>

        <DialogFooter className="justify-between gap-2 sm:justify-between">
          <div className="flex gap-2">
            <Button
              variant="outline"

              className="text-sm"
              disabled={submitMutation.isPending}
              onClick={close}
            >
              {t("cancel", { defaultValue: "Cancel" })}
            </Button>
          </div>
          <div className="flex gap-2">
            {step > 1 && (
              <Button
                variant="outline"

                className="gap-1.5"
                onClick={goBack}
                disabled={submitMutation.isPending}
              >
                <ChevronLeft className="h-3.5 w-3.5" />
                {t("back", { defaultValue: "Back" })}
              </Button>
            )}
            {step < 3 && (
              <Button

                className="gap-1.5"
                // Gate Next on step1Valid for every forward jump — going back
                // and clearing the URL must not let the user land on Finish.
                disabled={!step1Valid}
                onClick={goNext}
              >
                {t("next", { defaultValue: "Next" })}
                <ChevronRight className="h-3.5 w-3.5" />
              </Button>
            )}
            {step === 3 && (
              <Button

                className="text-sm"
                disabled={!step3Valid || submitMutation.isPending || !step1Valid}
                onClick={() => {
                  if (!submitMutation.isPending && step1Valid && step3Valid) submitMutation.mutate(draft, {
                    onSuccess: () => { onOpenChange(false); onCompleted?.(); },
                  });
                }}
              >
                {t("observability.quickStart.finish", { defaultValue: "Finish" })}
              </Button>
            )}
          </div>
        </DialogFooter>
        {discardDialog}
      </DialogContent>
    </Dialog>
  );
};

export default QuickStartWizard;
