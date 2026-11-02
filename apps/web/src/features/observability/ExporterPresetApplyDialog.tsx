import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Sparkles } from "lucide-react";

import { useApiMutation } from "@/shared/api/hooks";
import type { ApiError } from "@/shared/api/client";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import {
  applyExporterPreset,
  listMonitoringPresets,
  type ExporterPresetCatalogItem,
  type QuerySourceItem,
} from "@/features/observability/api";
import ScopeFields from "./ScopeFields";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import { type MonitoringScopeType } from "./scope";
import type { ScopeEntities } from "./useScopeEntities";
import {
  METRIC_QUERIES_KEY,
  METRIC_SETS_KEY,
  METRIC_SET_ASSIGNMENTS_KEY,
  PRESETS_KEY,
  QUERY_SOURCE_ASSIGNMENTS_KEY,
  invalidateInspectorMetricCaches,
} from "./keys";

interface DraftState {
  exporterSlug: string;
  querySourceId: string;
  scopeType: MonitoringScopeType;
  scopeId: string;
}

const EMPTY_DRAFT: DraftState = {
  exporterSlug: "",
  querySourceId: "",
  scopeType: "global",
  scopeId: "",
};

interface ExporterPresetApplyDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Existing query sources the apply call must reference. */
  sources: QuerySourceItem[];
  entities: ScopeEntities;
}

/**
 * "Apply preset" dialog. Lets the user pick a built-in exporter from the
 * server-side preset catalog plus a scope-bound data source, then atomically
 * materialises the preset's metric queries, metric set, metric-set
 * assignment, AND a `monitoring_query_source_assignment` so the preset reads
 * from the chosen source.
 */
const ExporterPresetApplyDialog = ({
  open,
  onOpenChange,
  sources,
  entities,
}: ExporterPresetApplyDialogProps) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const [draft, setDraft] = useState<DraftState>(EMPTY_DRAFT);

  const formKey = open ? "open" : "closed";
  const [prevFormKey, setPrevFormKey] = useState(formKey);
  if (prevFormKey !== formKey) {
    setPrevFormKey(formKey);
    setDraft(EMPTY_DRAFT);
  }

  const set = <K extends keyof DraftState>(key: K, value: DraftState[K]) =>
    { if (!applyMutation.isPending) setDraft((prev) => ({ ...prev, [key]: value })); };

  const presetsQuery = useQuery({
    queryKey: PRESETS_KEY,
    queryFn: () => listMonitoringPresets(),
    enabled: open,
  });

  const presets = useMemo<ExporterPresetCatalogItem[]>(
    () => presetsQuery.data?.items ?? [],
    [presetsQuery.data],
  );

  const applyMutation = useApiMutation({
    mutationFn: (input: DraftState) =>
      applyExporterPreset(input.exporterSlug, {
        query_source_id: input.querySourceId,
        scope_type: input.scopeType,
        scope_id: input.scopeType === "global" ? null : input.scopeId || null,
      }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: METRIC_QUERIES_KEY });
      void qc.invalidateQueries({ queryKey: METRIC_SETS_KEY });
      void qc.invalidateQueries({ queryKey: METRIC_SET_ASSIGNMENTS_KEY });
      void qc.invalidateQueries({ queryKey: QUERY_SOURCE_ASSIGNMENTS_KEY });
      invalidateInspectorMetricCaches(qc);
      notify(
        "success",
        t("observability.preset.applied", { defaultValue: "Preset applied" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const canSubmit =
    !!draft.exporterSlug &&
    !!draft.querySourceId &&
    (draft.scopeType === "global" || !!draft.scopeId) &&
    !applyMutation.isPending;

  const noSources = sources.length === 0;
  const dirty = JSON.stringify(draft) !== JSON.stringify(EMPTY_DRAFT);
  const { confirmExit, discardDialog } = useUnsavedChangesGuard(open && dirty);
  const close = () => {
    if (applyMutation.isPending) return;
    if (dirty) confirmExit(() => onOpenChange(false));
    else onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={(next) => { if (!next) close(); }}>
      <DialogContent
        className="max-w-lg"
        description={t("observability.preset.dialogHint", {
          defaultValue:
            "Apply a built-in exporter preset: this creates the matching metric queries, a metric set, and a scope assignment in one step.",
        })}
      >
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <Sparkles className="h-4 w-4 text-primary" />
            {t("observability.preset.applyTitle", { defaultValue: "Apply preset" })}
          </DialogTitle>
        </DialogHeader>

        <fieldset disabled={applyMutation.isPending} className="grid min-w-0 gap-4">
          <FormField
            label={t("observability.preset.exporter", { defaultValue: "Exporter preset" })}
            htmlFor="apply-preset-exporter"
            required
            help={
              presetsQuery.isPending
                ? t("loading", { defaultValue: "Loading…" })
                : presets.length === 0
                  ? t("observability.preset.exporterEmpty", {
                      defaultValue: "No exporter catalog entries available.",
                    })
                  : undefined
            }
          >
            <FormFieldControl><EntitySelect
              id="apply-preset-exporter"
              disabled={applyMutation.isPending}
              value={draft.exporterSlug}
              onValueChange={(v) => set("exporterSlug", v)}
              triggerClassName="text-sm"
              placeholder={t("observability.preset.exporterPlaceholder", {
                defaultValue: "Pick an exporter…",
              })}
              searchPlaceholder={t("search")}
              items={presets.map((preset) => ({
                id: preset.exporter_slug,
                label: preset.description
                  ? `${preset.name} — ${preset.description}`
                  : preset.name,
              }))}
            /></FormFieldControl>
          </FormField>

          <FormField
            label={t("observability.preset.querySource", { defaultValue: "Data source" })}
            htmlFor="apply-preset-source"
            required
            help={
              noSources
                ? t("observability.preset.querySourceEmpty", {
                    defaultValue:
                      "Add a data source first so preset queries have somewhere to read from.",
                  })
                : t("observability.preset.querySourceHint", {
                    defaultValue:
                      "The preset is wired to this source at the chosen scope so its queries actually run.",
                  })
            }
          >
            <FormFieldControl><EntitySelect
              value={draft.querySourceId}
              id="apply-preset-source"
              onValueChange={(v) => set("querySourceId", v)}
              disabled={noSources || applyMutation.isPending}
              triggerClassName="text-sm"
              placeholder={t("observability.preset.querySourcePlaceholder", {
                defaultValue: "Pick a data source…",
              })}
              searchPlaceholder={t("search")}
              items={sources.map((source) => ({ id: source.id, label: source.name }))}
            /></FormFieldControl>
          </FormField>

          <ScopeFields
            disabled={applyMutation.isPending}
            scopeType={draft.scopeType}
            scopeId={draft.scopeId}
            onScopeTypeChange={(v) => set("scopeType", v)}
            onScopeIdChange={(v) => set("scopeId", v)}
            entities={entities}
          />
        </fieldset>

        <DialogFooter>
          <Button
            variant="outline"

            className="text-sm"
            disabled={applyMutation.isPending}
            onClick={close}
          >
            {t("cancel", { defaultValue: "Cancel" })}
          </Button>
          <Button

            className="text-sm"
            disabled={!canSubmit}
            onClick={() => { if (canSubmit) applyMutation.mutate(draft, { onSuccess: () => onOpenChange(false) }); }}
          >
            {t("observability.preset.apply", { defaultValue: "Apply preset" })}
          </Button>
        </DialogFooter>
        {discardDialog}
      </DialogContent>
    </Dialog>
  );
};

export default ExporterPresetApplyDialog;
