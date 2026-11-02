import { isUuid } from "@/shared/lib/ids";
import { isChoiceFieldType, isMultiFieldType, type ChoiceSet, type CustomFieldDefinition } from "@/features/inventory/custom-fields-api";
import { CustomFieldValidationError } from "./customFieldErrors";

export const SECRET_MASK = "****";
const DATETIME_ZONE_SUFFIX = /(?:Z|[+-]\d\d:\d\d)$/;
export type DraftValue = string | boolean | string[];
/** Ephemeral authoring state. Create stacks keep one independent state per frame. */
export interface EntityCustomFieldDraft {
  values: Record<string, DraftValue>;
  dirty: string[];
  validated: boolean;
}
export const emptyCustomFieldDraft = (): EntityCustomFieldDraft => ({ values: {}, dirty: [], validated: false });

export function buildCreateCustomFields(handle: { buildPayload: () => Record<string, unknown> | undefined } | null): Record<string, unknown> | undefined {
  if (!handle) throw new CustomFieldValidationError("", "unavailable");
  return handle.buildPayload();
}

function toCustomFieldDraft(def: CustomFieldDefinition, raw: unknown, create: boolean): DraftValue {
  if (def.is_secret) return "";
  if (def.field_type === "boolean") return raw === true;
  if (isMultiFieldType(def.field_type)) return Array.isArray(raw) ? raw.filter((value): value is string => typeof value === "string") : [];
  if (def.field_type === "json") return raw == null ? "" : JSON.stringify(raw, null, 2);
  // datetime-local cannot display a timezone suffix. Keep the instant while
  // showing its local wall time; serialization restores RFC3339 for the API.
  if (create && def.field_type === "datetime" && typeof raw === "string" && DATETIME_ZONE_SUFFIX.test(raw)) {
    const date = new Date(raw);
    if (!Number.isNaN(date.getTime())) {
      return new Date(date.getTime() - date.getTimezoneOffset() * 60_000).toISOString().slice(0, -1);
    }
  }
  if (typeof raw === "string") return raw;
  return typeof raw === "number" || typeof raw === "boolean" ? String(raw) : "";
}

export function customFieldDraftValues(definitions: CustomFieldDefinition[], fields: Record<string, unknown> | undefined, state: EntityCustomFieldDraft, create: boolean): Record<string, DraftValue> {
  return Object.fromEntries(definitions.map(def => {
    const initial = toCustomFieldDraft(def, create ? def.default_value : fields?.[def.name], create);
    // Read-only/hidden defaults are configuration, not an authoring permission.
    const value = create && def.ui_editable !== "yes" ? initial : state.values[def.name] ?? initial;
    return [def.name, value];
  }));
}

const missing = (value: DraftValue | undefined) => value === undefined || (typeof value === "string" ? value.trim() === "" : Array.isArray(value) && value.length === 0);

export function customFieldDraftErrors(definitions: CustomFieldDefinition[], values: Record<string, DraftValue>, state: EntityCustomFieldDraft, create: boolean, choices: Map<string, ChoiceSet>): Map<string, CustomFieldValidationError> {
  const errors = new Map<string, CustomFieldValidationError>();
  for (const def of definitions) {
    const supplied = state.dirty.includes(def.name);
    const value = values[def.name];
    if (create && def.required && def.ui_editable !== "yes" && (def.default_value == null || missing(value))) {
      errors.set(def.name, new CustomFieldValidationError("", "uneditable"));
      continue;
    }
    if (def.required && (create || supplied) && missing(value)) {
      errors.set(def.name, new CustomFieldValidationError(def.label, "required"));
      continue;
    }
    if (missing(value) || (def.is_secret && !supplied)) continue;
    if (def.is_secret && value === SECRET_MASK) {
      errors.set(def.name, new CustomFieldValidationError(def.label, "invalid"));
      continue;
    }
    if (def.field_type === "json") {
      try {
        const parsed: unknown = JSON.parse(String(value));
        if (def.required && (parsed === null || parsed === "" || (Array.isArray(parsed) && parsed.length === 0))) {
          errors.set(def.name, new CustomFieldValidationError(def.label, "required"));
        }
      } catch {
        errors.set(def.name, new CustomFieldValidationError(def.label));
      }
    }
    if (def.field_type === "integer" || def.field_type === "decimal") {
      const number = Number(value);
      if (!Number.isFinite(number) || (def.field_type === "integer" && !Number.isInteger(number)) ||
        (def.validation_minimum != null && number < def.validation_minimum) || (def.validation_maximum != null && number > def.validation_maximum)) {
        errors.set(def.name, new CustomFieldValidationError(def.label, "invalid"));
      }
    }
    if ((def.field_type === "object" && !isUuid(value)) || (def.field_type === "multiobject" && (!Array.isArray(value) || value.some(id => !isUuid(id))))) {
      errors.set(def.name, new CustomFieldValidationError(def.label, "invalid"));
    }
    if (isChoiceFieldType(def.field_type)) {
      const allowed = new Set(choices.get(def.choice_set_id ?? "")?.extra_choices.map(choice => choice.value) ?? []);
      if ((Array.isArray(value) ? value : [String(value)]).some(choice => !allowed.has(choice))) errors.set(def.name, new CustomFieldValidationError(def.label, "invalid"));
    }
  }
  return errors;
}

/** Edit keeps the established full non-secret projection and only edited secrets;
 * create includes required values and operator edits, never optional untouched secrets. */
export function buildCustomFieldDraftPayload(definitions: CustomFieldDefinition[], values: Record<string, DraftValue>, state: EntityCustomFieldDraft, create: boolean, choices: Map<string, ChoiceSet>): Record<string, unknown> | undefined {
  if (!create && state.dirty.length === 0) return undefined;
  const errors = customFieldDraftErrors(definitions, values, state, create, choices);
  const first = errors.values().next().value;
  if (first) throw first;
  const out: Record<string, unknown> = {};
  for (const def of definitions) {
    const dirty = state.dirty.includes(def.name);
    if ((def.is_secret && !dirty) || (create && !def.required && (!dirty || def.ui_editable !== "yes"))) continue;
    const value = values[def.name];
    if (def.is_secret) { out[def.name] = typeof value === "string" ? value : ""; continue; }
    if (def.field_type === "boolean") out[def.name] = value === true;
    else if (isMultiFieldType(def.field_type)) out[def.name] = Array.isArray(value) ? value : [];
    else if (typeof value !== "string" || value.trim() === "") {
      if (create && dirty) out[def.name] = null;
    } else if (def.field_type === "json") out[def.name] = JSON.parse(value);
    else if (def.field_type === "integer" || def.field_type === "decimal") out[def.name] = Number(value);
    else if (def.field_type === "datetime" && (create || dirty)) {
      const originalDefault = def.default_value;
      // A zone-less wall time cannot distinguish the two DST overlap instants.
      const preserveDefault = create && (!dirty || def.ui_editable !== "yes") &&
        typeof originalDefault === "string" && DATETIME_ZONE_SUFFIX.test(originalDefault) &&
        value === toCustomFieldDraft(def, originalDefault, true);
      const local = preserveDefault ? originalDefault : value.length === 16 ? `${value}:00` : value;
      const date = new Date(local);
      if (Number.isNaN(date.getTime())) throw new CustomFieldValidationError(def.label, "invalid");
      out[def.name] = preserveDefault ? originalDefault : date.toISOString();
    } else out[def.name] = value;
  }
  return Object.keys(out).length > 0 ? out : undefined;
}
