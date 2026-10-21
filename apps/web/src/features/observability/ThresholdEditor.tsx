import { useTranslation } from "react-i18next";
import { useId } from "react";

import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import FormSection from "@/shared/ui-shared/FormSection";
import MetricLabel from "./MetricLabel";
import type { HostLabelMetric } from "./api";
import {
  normalizeThresholdDirection,
  normalizeThresholdMode,
  statusFromThresholdValue,
  validateThresholdValue,
  type ThresholdEditorValue,
} from "./thresholdValue";

const STATUS_MODES: ThresholdEditorValue["mode"][] = [
  "auto",
  "none",
  "numeric",
  "state",
];
const NUMERIC_DIRECTIONS: ThresholdEditorValue["direction"][] = [
  "higher_is_worse",
  "lower_is_worse",
];

type JsonRecord = Record<string, unknown>;

interface ThresholdEditorProps {
  value: ThresholdEditorValue;
  onChange: (next: ThresholdEditorValue) => void;
  /** Optional live-preview controls. Both must be supplied together. */
  preview?: {
    label: string;
    unit: string | null;
    /** Raw value from /latest (already in instant-vector envelope). */
    sampleValue?: unknown;
    /** Hand-typed sample fallback when no live latest value is available. */
    sampleText: string;
    onSampleTextChange: (next: string) => void;
    /** When true, hide the sample text input — value is fully derived. */
    hideSampleInput?: boolean;
  };
  /** Render only the threshold fields without the section frame/header. */
  bare?: boolean;
  disabled?: boolean;
}

function buildPreviewMetric(
  label: string,
  unit: string | null,
  value: unknown,
  status: JsonRecord,
): HostLabelMetric {
  return {
    query: "__preview__",
    value,
    label: label || "preview",
    unit,
    value_type: null,
    widget_type: null,
    list_render: "value",
    options: { display: { status } },
  };
}

function promInstantVector(raw: string): unknown {
  return {
    status: "success",
    data: {
      resultType: "vector",
      result: [{ metric: {}, value: [Math.floor(Date.now() / 1000), raw] }],
    },
  };
}

const ThresholdEditor = ({ value, onChange, preview, bare, disabled = false }: ThresholdEditorProps) => {
  const { t } = useTranslation();
  const fieldId = useId();
  const set = <K extends keyof ThresholdEditorValue>(
    key: K,
    next: ThresholdEditorValue[K],
  ) => { if (!disabled) onChange({ ...value, [key]: next }); };

  const validationError = validateThresholdValue(value, t);
  const numericOrState =
    value.mode === "numeric" ? "numeric" : value.mode === "state" ? "state" : null;
  const setNumericOrState = (next: "numeric" | "state") => set("mode", next);

  const previewSample =
    preview?.sampleValue !== undefined
      ? preview.sampleValue
      : preview && preview.sampleText.trim()
        ? promInstantVector(preview.sampleText.trim())
        : undefined;
  const previewStatus = statusFromThresholdValue(value);

  const body = (
    <fieldset disabled={disabled} className="grid min-w-0 gap-4">
      <FormSelect
        disabled={disabled}
        label={t("observability.metric.statusMode", { defaultValue: "Color rule" })}
        value={value.mode}
        onValueChange={(next) => set("mode", normalizeThresholdMode(next))}
        help={t("observability.metric.statusModeHint", {
          defaultValue:
            "Auto colors states and percentages. Use numeric thresholds for warning/critical ranges, or no color for neutral counters.",
        })}
        options={STATUS_MODES.map((mode) => ({
          value: mode,
          label: t(`observability.metric.statusModes.${mode}`, { defaultValue: mode }),
        }))}
      />

      {/* Numeric vs State segmented control. Visible only when the operator
          has already picked a custom rule mode — auto/none keeps the surface
          quiet. */}
      {(value.mode === "numeric" || value.mode === "state") && (
        <div
          role="tablist"
          aria-label={t("observability.metric.customRuleMode", {
            defaultValue: "Threshold rule mode",
          })}
          className="inline-flex rounded-md border border-border bg-background p-0.5 text-xs"
        >
          {(["numeric", "state"] as const).map((mode) => (
            <button
              key={mode}
              type="button"
              role="tab"
              aria-selected={numericOrState === mode}
              className={
                numericOrState === mode
                  ? "rounded-sm bg-accent px-3 py-1 font-medium text-accent-foreground"
                  : "rounded-sm px-3 py-1 text-muted-foreground hover:text-foreground"
              }
              onClick={() => setNumericOrState(mode)}
            >
              {t(`observability.metric.statusModes.${mode}`, { defaultValue: mode })}
            </button>
          ))}
        </div>
      )}

      {value.mode === "numeric" && (
        <>
          <FormSelect
            disabled={disabled}
            label={t("observability.metric.thresholdDirection", {
              defaultValue: "Direction",
            })}
            value={value.direction}
            onValueChange={(next) => set("direction", normalizeThresholdDirection(next))}
            options={NUMERIC_DIRECTIONS.map((direction) => ({
              value: direction,
              label: t(`observability.metric.directions.${direction}`, {
                defaultValue:
                  direction === "higher_is_worse" ? "Higher is worse" : "Lower is worse",
              }),
            }))}
          />
          <div className="grid grid-cols-2 gap-3">
            <FormField
              htmlFor={`${fieldId}-warning`}
              label={
                <span className="flex items-center gap-2">
                  <span className="h-2 w-2 rounded-full bg-status-warning" />
                  {t("observability.metric.warningThreshold", { defaultValue: "Warning" })}
                </span>
              }
            >
              <Input
                id={`${fieldId}-warning`}
                inputMode="decimal"
                value={value.warning}
                onChange={(e) => set("warning", e.target.value)}
              />
            </FormField>
            <FormField
              htmlFor={`${fieldId}-critical`}
              label={
                <span className="flex items-center gap-2">
                  <span className="h-2 w-2 rounded-full bg-status-negative" />
                  {t("observability.metric.criticalThreshold", { defaultValue: "Critical" })}
                </span>
              }
            >
              <Input
                id={`${fieldId}-critical`}
                inputMode="decimal"
                value={value.critical}
                onChange={(e) => set("critical", e.target.value)}
              />
            </FormField>
            <p className="col-span-2 text-xs text-muted-foreground">
              {t("observability.metric.thresholdHint", {
                defaultValue:
                  "Thresholds compare the raw numeric query result before display unit conversion.",
              })}
            </p>
            {validationError ? (
              <p className="col-span-2 text-xs text-status-negative">{validationError}</p>
            ) : null}
          </div>
        </>
      )}

      {value.mode === "state" && (
        <div className="grid gap-2">
          <FormField
            htmlFor={`${fieldId}-normal-states`}
            label={t("observability.metric.normalStates", { defaultValue: "Normal values" })}
            labelClassName="text-status-positive"
          >
            <Input
              id={`${fieldId}-normal-states`}
              value={value.normalStates}
              onChange={(e) => set("normalStates", e.target.value)}
            />
          </FormField>
          <FormField
            htmlFor={`${fieldId}-warning-states`}
            label={t("observability.metric.warningStates", { defaultValue: "Warning values" })}
            labelClassName="text-status-warning"
          >
            <Input
              id={`${fieldId}-warning-states`}
              value={value.warningStates}
              onChange={(e) => set("warningStates", e.target.value)}
            />
          </FormField>
          <FormField
            htmlFor={`${fieldId}-critical-states`}
            label={t("observability.metric.criticalStates", { defaultValue: "Critical values" })}
            labelClassName="text-status-negative"
          >
            <Input
              id={`${fieldId}-critical-states`}
              value={value.criticalStates}
              onChange={(e) => set("criticalStates", e.target.value)}
            />
          </FormField>
        </div>
      )}

      {preview ? (
        <div className="rounded-md border border-border bg-background p-2.5">
          <div className="flex items-center justify-between gap-2">
            <span className="text-xs font-medium text-muted-foreground">
              {t("observability.metric.preview", { defaultValue: "Preview" })}
            </span>
            {previewSample !== undefined ? (
              <MetricLabel
                metric={buildPreviewMetric(preview.label, preview.unit, previewSample, previewStatus)}
              />
            ) : (
              <span className="text-xs text-muted-foreground">
                {t("observability.metric.previewEmpty", {
                  defaultValue: "Type a sample value to preview",
                })}
              </span>
            )}
          </div>
          {!preview.hideSampleInput ? (
            <div className="mt-2 flex items-center gap-2">
              <label htmlFor={`${fieldId}-sample`} className="shrink-0 text-xs text-muted-foreground">
                {t("observability.metric.previewSample", { defaultValue: "Sample value" })}
              </label>
              <Input
                id={`${fieldId}-sample`}
                value={preview.sampleText}
                onChange={(e) => { if (!disabled) preview.onSampleTextChange(e.target.value); }}
                placeholder={value.mode === "state" ? "down" : "92"}
              />
              <Button
                type="button"
                variant="ghost"
                onClick={() => { if (!disabled) preview.onSampleTextChange(""); }}
              >
                {t("observability.metric.previewClear", { defaultValue: "Clear" })}
              </Button>
            </div>
          ) : null}
        </div>
      ) : null}
    </fieldset>
  );

  if (bare) return body;

  return (
    <FormSection
      title={t("observability.metric.statusAndThresholdsTitle", {
            defaultValue: "Status and thresholds",
          })}
      description={t("observability.metric.statusAndThresholdsHint", {
            defaultValue:
              "Pick when the value turns warning or critical, and preview the colored pill the list and inspector will render.",
          })}
    >
      {body}
    </FormSection>
  );
};

export default ThresholdEditor;
