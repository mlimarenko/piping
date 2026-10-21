// Pure helpers for the threshold editor. Kept in their own module so the
// component file in ThresholdEditor.tsx can stay react-refresh-clean (a
// .tsx file may only export React components for fast-refresh to work).

export interface ThresholdEditorValue {
  mode: "auto" | "none" | "numeric" | "state";
  direction: "higher_is_worse" | "lower_is_worse";
  warning: string;
  critical: string;
  normalStates: string;
  warningStates: string;
  criticalStates: string;
}

export const THRESHOLD_EDITOR_DEFAULTS: ThresholdEditorValue = {
  mode: "auto",
  direction: "higher_is_worse",
  warning: "",
  critical: "",
  normalStates: "up, 1, true, ok, active",
  warningStates: "unknown, pending",
  criticalStates: "down, 0, false, failed, error",
};

const STATUS_MODES: ThresholdEditorValue["mode"][] = [
  "auto",
  "none",
  "numeric",
  "state",
];

type JsonRecord = Record<string, unknown>;

function asRecord(value: unknown): JsonRecord {
  return value && typeof value === "object" && !Array.isArray(value)
    ? { ...(value as JsonRecord) }
    : {};
}

function csv(value: unknown, fallback: string): string {
  if (Array.isArray(value)) {
    const text = value
      .map((item) =>
        typeof item === "string" || typeof item === "number" || typeof item === "boolean"
          ? String(item)
          : "",
      )
      .filter(Boolean)
      .join(", ");
    return text || fallback;
  }
  return typeof value === "string" && value.trim() ? value : fallback;
}

function numberString(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? String(value) : "";
}

function tokens(value: string): string[] {
  return value
    .split(",")
    .map((token) => token.trim())
    .filter(Boolean);
}

function optionalNumber(value: string): number | null {
  if (!value.trim()) return null;
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) ? parsed : null;
}

export function normalizeThresholdMode(value: unknown): ThresholdEditorValue["mode"] {
  return STATUS_MODES.includes(value as ThresholdEditorValue["mode"])
    ? (value as ThresholdEditorValue["mode"])
    : "auto";
}

export function normalizeThresholdDirection(
  value: unknown,
): ThresholdEditorValue["direction"] {
  return value === "lower_is_worse" ? "lower_is_worse" : "higher_is_worse";
}

/** Parse the `display.status` subtree into editor state. */
export function thresholdValueFromStatus(status: unknown): ThresholdEditorValue {
  const record = asRecord(status);
  return {
    mode: normalizeThresholdMode(record.mode),
    direction: normalizeThresholdDirection(record.direction),
    warning: numberString(record.warning),
    critical: numberString(record.critical),
    normalStates: csv(record.normal, THRESHOLD_EDITOR_DEFAULTS.normalStates),
    warningStates: csv(
      record.warning_states ??
        record.warningValues ??
        (Array.isArray(record.warning) || typeof record.warning === "string"
          ? record.warning
          : undefined),
      THRESHOLD_EDITOR_DEFAULTS.warningStates,
    ),
    criticalStates: csv(
      record.critical_states ??
        record.criticalValues ??
        (Array.isArray(record.critical) || typeof record.critical === "string"
          ? record.critical
          : undefined),
      THRESHOLD_EDITOR_DEFAULTS.criticalStates,
    ),
  };
}

/** Serialize the editor state into the `display.status` subtree. */
export function statusFromThresholdValue(value: ThresholdEditorValue): JsonRecord {
  const status: JsonRecord = { mode: value.mode };
  if (value.mode === "numeric") {
    status.direction = value.direction;
    status.warning = optionalNumber(value.warning);
    status.critical = optionalNumber(value.critical);
  }
  if (value.mode === "state") {
    status.normal = tokens(value.normalStates);
    status.warning = tokens(value.warningStates);
    status.critical = tokens(value.criticalStates);
  }
  return status;
}

/**
 * Returns a localized validation message, or `null` when the editor state
 * is consistent. Numeric warning/critical must respect the chosen direction.
 */
export function validateThresholdValue(
  value: ThresholdEditorValue,
  t: (key: string, opts?: Record<string, unknown>) => string,
): string | null {
  if (value.mode !== "numeric") return null;
  const warn = optionalNumber(value.warning);
  const crit = optionalNumber(value.critical);
  if (warn === null || crit === null) return null;
  if (value.direction === "higher_is_worse" && warn >= crit) {
    return t("observability.metric.thresholdInvalidHigher", {
      defaultValue: "Warning must be smaller than critical when higher values are worse.",
    });
  }
  if (value.direction === "lower_is_worse" && warn <= crit) {
    return t("observability.metric.thresholdInvalidLower", {
      defaultValue: "Warning must be larger than critical when lower values are worse.",
    });
  }
  return null;
}
