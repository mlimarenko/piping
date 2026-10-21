import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2 } from "lucide-react";

import type { ApiError } from "@/shared/api/client";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { notify } from "@/shared/lib/notifications";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
} from "@/shared/ui-primitives/sheet";
import FormField, { FormFieldControl } from "@/shared/ui-shared/FormField";
import { useUnsavedChangesGuard } from "@/shared/ui-shared/useUnsavedChangesGuard";
import {
  createMetricQueryHostOverride,
  deleteMetricQueryHostOverride,
  listMetricQueryHostOverrides,
  updateMetricQueryHostOverride,
  type MetricQueryHostOverrideItem,
  type MonitoringMetricDescriptor,
} from "@/features/observability/api";
import ThresholdEditor from "./ThresholdEditor";
import {
  statusFromThresholdValue,
  thresholdValueFromStatus,
  validateThresholdValue,
  type ThresholdEditorValue,
} from "./thresholdValue";
import { invalidateInspectorMetricCaches } from "./keys";

type JsonRecord = Record<string, unknown>;

function asRecord(value: unknown): JsonRecord {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...(value as JsonRecord) }
    : {};
}

interface HostMetricOverrideDrawerProps {
  open: boolean;
  onOpenChange: (next: boolean) => void;
  /** The host whose override is being edited. */
  host: { id: string; name: string };
  /**
   * The descriptor coming from the inspector — used for default label/unit
   * display and to seed the threshold editor when no override exists yet.
   * The descriptor already carries `metric_query_id` (Option<Uuid>); the
   * separate `metricQueryId` prop pins which id this drawer is editing.
   */
  metric: MonitoringMetricDescriptor;
  /** Required: the override join row keys on metric_query_id. */
  metricQueryId: string;
}

const HostMetricOverrideForm = ({
  open,
  onOpenChange,
  host,
  metric,
  metricQueryId,
}: HostMetricOverrideDrawerProps) => {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { can } = usePermissions();

  // The list endpoint returns ALL overrides on the host; we then filter to
  // the metric we're editing. There is no per-metric GET so this is the
  // canonical read path.
  const overridesQuery = useQuery({
    queryKey: ["observability", "host-overrides", host.id],
    queryFn: () => listMetricQueryHostOverrides(host.id),
    enabled: open,
    staleTime: 5_000,
  });

  const existing = useMemo<MetricQueryHostOverrideItem | null>(() => {
    const items = overridesQuery.data?.items ?? [];
    return (
      items.find((item) => item.metric_query_id === metricQueryId) ?? null
    );
  }, [overridesQuery.data, metricQueryId]);

  const seededDisplay = existing ? asRecord(existing.options_override) : asRecord(asRecord(metric.options).display);
  const [labelOverride, setLabelOverride] = useState(existing?.label_override ?? "");
  const [unitOverride, setUnitOverride] = useState(existing?.unit_override ?? "");
  const [threshold, setThresholdRaw] = useState<ThresholdEditorValue>(
    () => thresholdValueFromStatus(seededDisplay.status),
  );
  // Track whether the threshold has been edited from its seeded baseline.
  // `apply_host_overrides` replaces `options.display` WHOLESALE — so saving
  // a label-only edit alongside the seeded threshold would clobber any
  // sibling display keys (e.g. future `display.format`) the descriptor
  // grows. Only persist the override block when the operator actually
  // touched the threshold.
  const [thresholdDirty, setThresholdDirty] = useState(existing?.options_override != null);
  const setThreshold = (next: ThresholdEditorValue) => {
    if (isPending || !canSave) return;
    setThresholdDirty(true);
    setThresholdRaw(next);
  };
  const [previewSample, setPreviewSample] = useState("");
  // Two-stage revert confirm — destructive op should never fire on a
  // single click. First click arms; second click within ~3s commits.
  const [revertArmed, setRevertArmed] = useState(false);
  // Reseed when the drawer opens or the existing override changes.
  const seedKey = `${open ? "open" : "closed"}:${existing?.id ?? "new"}`;
  const [prevSeedKey, setPrevSeedKey] = useState(seedKey);
  if (prevSeedKey !== seedKey) {
    setPrevSeedKey(seedKey);
    if (existing) {
      setLabelOverride(existing.label_override ?? "");
      setUnitOverride(existing.unit_override ?? "");
      // options_override IS the new `display` subtree (apply_host_overrides
      // replaces `options.display` wholesale on the descriptor).
      const display = asRecord(existing.options_override);
      setThresholdRaw(thresholdValueFromStatus(display.status));
      // Existing override means a saved threshold IS the operator's intent
      // — keep it dirty so re-saving without touching it preserves it.
      setThresholdDirty(existing.options_override != null);
    } else {
      setLabelOverride("");
      setUnitOverride("");
      // Seed from the metric's own display rules so the editor isn't blank.
      const metricOptions = asRecord(metric.options);
      const metricDisplay = asRecord(metricOptions.display);
      setThresholdRaw(thresholdValueFromStatus(metricDisplay.status));
      setThresholdDirty(false);
    }
    setPreviewSample("");
    setRevertArmed(false);
  }

  const validationError = validateThresholdValue(threshold, t);

  const invalidate = () => {
    void qc.invalidateQueries({
      queryKey: ["observability", "host-overrides", host.id],
    });
    // Host labels + inspector latest caches read the descriptor whose options
    // are mutated by overrides — clear them so the new label/unit/colour shows
    // up immediately.
    invalidateInspectorMetricCaches(qc);
    void qc.invalidateQueries({ queryKey: ["monitoring", "host-labels"] });
  };

  const saveMutation = useApiMutation({
    mutationFn: () => {
      // Only persist the display-override block when the operator actually
      // edited the threshold. A `null` keeps the descriptor's own display
      // rules (label/unit overrides still apply on their own).
      const display: JsonRecord | null = thresholdDirty
        ? { status: statusFromThresholdValue(threshold) }
        : null;
      const body = {
        host_id: host.id,
        metric_query_id: metricQueryId,
        label_override: labelOverride.trim() || null,
        unit_override: unitOverride.trim() || null,
        options_override: display,
      };
      return existing
        ? updateMetricQueryHostOverride(existing.id, body)
        : createMetricQueryHostOverride(body);
    },
    onSuccess: () => {
      invalidate();
      notify(
        "success",
        t("observability.hostOverride.saved", { defaultValue: "Override saved" }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const deleteMutation = useApiMutation({
    mutationFn: () => {
      if (!existing) return Promise.resolve();
      return deleteMetricQueryHostOverride(existing.id);
    },
    onSuccess: () => {
      invalidate();
      notify(
        "success",
        t("observability.hostOverride.removed", {
          defaultValue: "Override removed",
        }),
      );
    },
    onError: (err: ApiError) => notify("error", err.message),
  });

  const isPending = saveMutation.isPending || deleteMutation.isPending;
  const canSave = can(existing ? "UPDATE" : "CREATE", "MONITORING");
  const dirty = labelOverride !== (existing?.label_override ?? "") ||
    unitOverride !== (existing?.unit_override ?? "") ||
    JSON.stringify(threshold) !== JSON.stringify(thresholdValueFromStatus(seededDisplay.status));
  const { confirmExit, discardDialog } = useUnsavedChangesGuard(open && dirty);
  const close = () => {
    if (isPending) return;
    if (dirty) confirmExit(() => onOpenChange(false));
    else onOpenChange(false);
  };
  const metricLabel = metric.label?.trim() || metric.query;

  return (
    <Sheet open={open} onOpenChange={(next) => { if (!next) close(); }}>
      <SheetContent side="right" className="flex w-full flex-col gap-4 sm:max-w-md">
        <SheetHeader>
          <SheetTitle>
            {t("observability.hostOverride.title", {
              defaultValue: "Override metric for host",
            })}
          </SheetTitle>
          <SheetDescription>
            {t("observability.hostOverride.description", {
              host: host.name,
              metric: metricLabel,
              defaultValue:
                "Per-host override of label, unit, and color rules for {{metric}} on {{host}}.",
            })}
          </SheetDescription>
        </SheetHeader>

        {overridesQuery.isPending ? (
          <div className="flex items-center gap-2 text-xs text-muted-foreground">
            <Loader2 className="h-3 w-3 animate-spin" />
            <span>{t("loading", { defaultValue: "Loading…" })}</span>
          </div>
        ) : (
          <fieldset disabled={isPending || !canSave} className="grid min-w-0 gap-4 overflow-y-auto pr-1">
            <FormField
              htmlFor="host-metric-label"
              label={t("observability.hostOverride.labelOverride", {
                defaultValue: "Label override",
              })}
              help={t("observability.hostOverride.labelOverrideHint", {
                defaultValue:
                  "Replaces the default metric label for this host only. Leave empty to keep the default.",
              })}
            >
              <FormFieldControl><Input
                id="host-metric-label"
                value={labelOverride}
                onChange={(e) => { if (!isPending && canSave) setLabelOverride(e.target.value); }}
                placeholder={metric.label ?? metric.query}
              /></FormFieldControl>
            </FormField>
            <FormField
              htmlFor="host-metric-unit"
              label={t("observability.hostOverride.unitOverride", {
                defaultValue: "Unit override",
              })}
              help={t("observability.hostOverride.unitOverrideHint", {
                defaultValue:
                  "Replaces the display unit for this host only. Leave empty to keep the default.",
              })}
            >
              <FormFieldControl><Input
                id="host-metric-unit"
                value={unitOverride}
                onChange={(e) => { if (!isPending && canSave) setUnitOverride(e.target.value); }}
                placeholder={metric.unit ?? ""}
              /></FormFieldControl>
            </FormField>

            <ThresholdEditor
              disabled={isPending || !canSave}
              value={threshold}
              onChange={setThreshold}
              preview={{
                label: labelOverride.trim() || metricLabel,
                unit: (unitOverride.trim() || metric.unit) ?? null,
                sampleText: previewSample,
                onSampleTextChange: (next) => { if (!isPending && canSave) setPreviewSample(next); },
              }}
            />
          </fieldset>
        )}

        <SheetFooter className="mt-auto">
          {existing && can("DELETE", "MONITORING") ? (
            <Button
              type="button"
              variant="outline"
              className={
                revertArmed
                  ? "border-status-negative/60 text-status-negative hover:bg-card"
                  : undefined
              }
              onClick={() => {
                if (isPending || !can("DELETE", "MONITORING")) return;
                if (!revertArmed) {
                  // First click: arm the revert, give the operator ~3s to
                  // change their mind. Single-click destructive ops are a
                  // foot-gun; the second-click rule is the smallest viable
                  // confirm without a modal.
                  setRevertArmed(true);
                  setTimeout(() => setRevertArmed(false), 3000);
                  return;
                }
                deleteMutation.mutate(undefined, { onSuccess: () => onOpenChange(false) });
              }}
              disabled={isPending}
              aria-label={
                revertArmed
                  ? t("observability.hostOverride.revertConfirm", {
                      defaultValue: "Confirm revert to default",
                    })
                  : t("observability.hostOverride.revert", {
                      defaultValue: "Revert to default",
                    })
              }
            >
              {revertArmed
                ? t("observability.hostOverride.revertConfirm", {
                    defaultValue: "Click again to confirm",
                  })
                : t("observability.hostOverride.revert", {
                    defaultValue: "Revert to default",
                  })}
            </Button>
          ) : null}
          <Button
            type="button"
            variant="ghost"
            onClick={close}
            disabled={isPending}
          >
            {t("cancel", { defaultValue: "Cancel" })}
          </Button>
          {canSave && <Button
            type="button"
            onClick={() => { if (canSave && !isPending && !validationError && overridesQuery.isSuccess) saveMutation.mutate(undefined, { onSuccess: () => onOpenChange(false) }); }}
            disabled={isPending || !!validationError || !overridesQuery.isSuccess}
          >
            {saveMutation.isPending ? (
              <Loader2 className="mr-1 h-3 w-3 animate-spin" />
            ) : null}
            {t("save", { defaultValue: "Save" })}
          </Button>}
        </SheetFooter>
        {discardDialog}
      </SheetContent>
    </Sheet>
  );
};

export default function HostMetricOverrideDrawer(props: HostMetricOverrideDrawerProps) {
  return <HostMetricOverrideForm key={`${props.host.id}:${props.metricQueryId}:${props.open}`} {...props} />;
}
