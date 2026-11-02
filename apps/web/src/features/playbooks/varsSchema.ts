/**
 * Single interpreter for a playbook's typed launch `vars_schema`.
 *
 * The backend stores `vars_schema` opaquely as a JSON Schema object and only
 * reads `properties.<name>.default` to seed extravars (see
 * `apps/api/src/app/workers.rs::vars_schema_defaults`). The UI is therefore the
 * sole authority for the richer launch-form shape, so authoring (editor) and
 * launch (run dialog) MUST round-trip through the helpers here — never parse the
 * schema ad-hoc in a component.
 *
 * Canonical mapping (UI row ⇄ JSON Schema property):
 *   { name, type, required, default, label?, description?, choices? }
 *     name     → key in `properties`
 *     type     → `type` ("choice" → `type: "string"` + `enum`)
 *     label    → `title`
 *     description → `description`
 *     default  → `default`
 *     choices  → `enum`
 *     required → membership of the top-level `required` array
 *
 * Secrets never belong here: the backend rejects plaintext secret keys in
 * `vars_schema` (`validate_no_plaintext_secret_keys`); secrets flow through the
 * credential picker instead.
 */

export type VarType = "string" | "number" | "boolean" | "choice";

export const VAR_TYPES: VarType[] = ["string", "number", "boolean", "choice"];

/** One declared launch variable, the UI-facing authoring/launch row shape. */
export interface VarSpec {
  name: string;
  type: VarType;
  required: boolean;
  /** Author-declared default, kept as the typed runtime value. */
  default: string | number | boolean | null;
  /** Human label shown on the launch form (falls back to `name`). */
  label?: string;
  /** Human help text shown under the launch/editor field. */
  description?: string;
  /** Allowed values when `type === "choice"`. */
  choices?: string[];
}

/** A flat extravars object: `{ <name>: <value> }`. */
export type Extravars = Record<string, string | number | boolean>;

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  return value.filter((v): v is string => typeof v === "string");
}

function parseType(spec: Record<string, unknown>): VarType {
  if (Array.isArray(spec.enum)) return "choice";
  const raw = spec.type;
  if (raw === "number" || raw === "integer") return "number";
  if (raw === "boolean") return "boolean";
  return "string";
}

function parseDefault(
  type: VarType,
  raw: unknown,
): string | number | boolean | null {
  if (raw === undefined || raw === null) return null;
  if (type === "boolean") return raw === true;
  if (type === "number") return typeof raw === "number" ? raw : null;
  // string / choice: a well-formed schema stores a string; anything else is
  // dropped rather than coerced (no `[object Object]` defaults).
  return typeof raw === "string" ? raw : null;
}

/**
 * Parse a stored `vars_schema` JSON Schema object into ordered UI rows.
 * Unknown / malformed input yields an empty list (no typed variables).
 */
export function parseVarsSchema(schema: unknown): VarSpec[] {
  if (schema === null || typeof schema !== "object" || Array.isArray(schema)) {
    return [];
  }
  const obj = schema as Record<string, unknown>;
  const properties = obj.properties;
  if (
    properties === null ||
    typeof properties !== "object" ||
    Array.isArray(properties)
  ) {
    return [];
  }
  const required = new Set(asStringArray(obj.required));
  const rows: VarSpec[] = [];
  for (const [name, rawSpec] of Object.entries(
    properties as Record<string, unknown>,
  )) {
    if (rawSpec === null || typeof rawSpec !== "object" || Array.isArray(rawSpec)) {
      continue;
    }
    const spec = rawSpec as Record<string, unknown>;
    const type = parseType(spec);
    const row: VarSpec = {
      name,
      type,
      required: required.has(name),
      default: parseDefault(type, spec.default),
    };
    if (typeof spec.title === "string" && spec.title.trim()) {
      row.label = spec.title;
    }
    if (typeof spec.description === "string" && spec.description.trim()) {
      row.description = spec.description;
    }
    if (type === "choice") {
      row.choices = asStringArray(spec.enum);
    }
    rows.push(row);
  }
  return rows;
}

/**
 * Serialize UI rows back into a `vars_schema` JSON Schema object, or `null` when
 * no (named) variables are declared so the backend clears the schema.
 */
export function serializeVarsSchema(rows: VarSpec[]): Record<string, unknown> | null {
  const properties: Record<string, unknown> = {};
  const required: string[] = [];
  for (const row of rows) {
    const name = row.name.trim();
    if (!name) continue;
    const property: Record<string, unknown> = {};
    if (row.type === "choice") {
      property.type = "string";
      const choices = (row.choices ?? []).map((c) => c.trim()).filter(Boolean);
      property.enum = choices;
    } else {
      property.type = row.type;
    }
    if (row.label && row.label.trim()) {
      property.title = row.label.trim();
    }
    if (row.description && row.description.trim()) {
      property.description = row.description.trim();
    }
    if (row.default !== null && row.default !== "") {
      property.default = row.default;
    }
    properties[name] = property;
    if (row.required) required.push(name);
  }
  if (Object.keys(properties).length === 0) return null;
  const schema: Record<string, unknown> = { type: "object", properties };
  if (required.length > 0) schema.required = required;
  return schema;
}

/** The typed launch value for a variable: its default, or a type-sane empty. */
export function defaultLaunchValue(spec: VarSpec): string | number | boolean {
  if (spec.default !== null) return spec.default;
  if (spec.type === "boolean") return false;
  // Number fields start empty (a string the input edits); coerced on submit.
  if (spec.type === "number") return "";
  if (spec.type === "choice") return spec.choices?.[0] ?? "";
  return "";
}

export interface VarsFormErrors {
  [name: string]: string;
}

/**
 * Validate the launch-form values against the declared variables. Returns a
 * map of `name → error-key`. Empty map means valid.
 */
export function validateVarsForm(
  specs: VarSpec[],
  values: Record<string, string | number | boolean>,
): VarsFormErrors {
  const errors: VarsFormErrors = {};
  for (const spec of specs) {
    const value = values[spec.name];
    const isEmpty =
      value === undefined ||
      value === null ||
      (typeof value === "string" && value.trim() === "");
    if (spec.required && isEmpty) {
      errors[spec.name] = "required";
      continue;
    }
    if (!isEmpty && spec.type === "number") {
      const num = typeof value === "number" ? value : Number(value);
      if (!Number.isFinite(num)) errors[spec.name] = "number";
    }
  }
  return errors;
}

/**
 * Coerce the launch-form values into a flat extravars object, dropping blank
 * optionals and casting numbers. The result is merged under any raw-extravars
 * escape hatch (raw wins) before submission.
 */
export function varsFormToExtravars(
  specs: VarSpec[],
  values: Record<string, string | number | boolean>,
): Extravars {
  const extravars: Extravars = {};
  for (const spec of specs) {
    const value = values[spec.name];
    const isEmpty =
      value === undefined ||
      value === null ||
      (typeof value === "string" && value.trim() === "");
    if (isEmpty) continue;
    if (spec.type === "number") {
      const num = typeof value === "number" ? value : Number(value);
      if (Number.isFinite(num)) extravars[spec.name] = num;
    } else if (spec.type === "boolean") {
      extravars[spec.name] = value === true || value === "true";
    } else {
      extravars[spec.name] = value;
    }
  }
  return extravars;
}
