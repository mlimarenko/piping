import { parseDocument, stringify } from "yaml";
import type { JobTemplateVarsSchema, ProjectEntry, VarsSchemaSource } from "./api";

export const NO_PARAMETERS_SCHEMA = { type: "object", properties: {}, additionalProperties: false } as const;

export function jobVarsSchema(source: VarsSchemaSource | undefined, resolved?: JobTemplateVarsSchema): unknown {
  if (!source || source.kind === "none") return NO_PARAMETERS_SCHEMA;
  if (source.kind === "inline") return source.schema;
  return resolved?.schema;
}

export function projectFileVarsSchema(entries: ProjectEntry[], path: string): unknown {
  const entry = entries.find((item) => item.path === path);
  if (entry?.content.kind !== "file") throw new Error("missing-schema-file");
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(new Uint8Array(entry.content.bytes)));
}

export type JsonValue = null | boolean | number | string | JsonValue[] | { [key: string]: JsonValue };
export type Extravars = Record<string, JsonValue>;
export type VarType = "string" | "number" | "integer" | "boolean" | "array" | "object" | "null";

export interface VarSpec {
  name: string;
  type: VarType;
  nullable: boolean;
  required: boolean;
  label: string;
  description?: string;
  choices?: JsonValue[];
  schema: Record<string, unknown>;
}

export function isObject(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function isJsonValue(value: unknown): value is JsonValue {
  if (value === null || typeof value === "string" || typeof value === "boolean") return true;
  if (typeof value === "number") return Number.isFinite(value);
  if (Array.isArray(value)) return value.every(isJsonValue);
  return isObject(value) && Object.values(value).every(isJsonValue);
}

export function parseParameterValues(text: string, format: "json" | "yaml" = "json"): Extravars {
  let value: unknown;
  if (format === "json") value = JSON.parse(text);
  else {
    const document = parseDocument(text, { uniqueKeys: true });
    if (document.errors.length) throw new Error("invalid-document");
    value = document.toJS({ maxAliasCount: 0 });
  }
  if (!isObject(value) || !isJsonValue(value)) throw new Error("object-required");
  return value as Extravars;
}

export function formatParameterValues(value: Extravars, format: "json" | "yaml"): string {
  return format === "json" ? JSON.stringify(value, null, 2) : stringify(value);
}

/** JSON Schema remains intact; the rows are only its form projection. */
export function parseVarsSchema(schema: unknown): VarSpec[] {
  if (!isObject(schema) || !isObject(schema.properties)) return [];
  const required = new Set(Array.isArray(schema.required) ? schema.required : []);
  return Object.entries(schema.properties).flatMap(([name, property]) => {
    if (!isObject(property)) return [];
    const types = Array.isArray(property.type) ? property.type : [property.type];
    const type = types.find((item) => item !== "null") ?? "null";
    return [{
      name,
      type: type as VarType,
      nullable: types.includes("null"),
      required: required.has(name),
      label: typeof property.title === "string" ? property.title : name,
      description: typeof property.description === "string" ? property.description : undefined,
      choices: Array.isArray(property.enum) ? property.enum.filter(isJsonValue) : undefined,
      schema: property,
    }];
  });
}

export function isSecretParameter(spec: VarSpec): boolean {
  return spec.schema.writeOnly === true && spec.schema.format === "secret-reference";
}

/** Secret declarations are references to the template's secret_refs, not values. */
function secretParameterNames(schema: unknown): Set<string> {
  return new Set(parseVarsSchema(schema).filter(isSecretParameter).map((spec) => spec.name));
}

export function parameterDefaults(schema: unknown): Extravars {
  return Object.fromEntries(parseVarsSchema(schema).filter((spec) => !isSecretParameter(spec) && Object.hasOwn(spec.schema, "default") && isJsonValue(spec.schema.default)).map((spec) => [spec.name, spec.schema.default as JsonValue]));
}

export function effectiveParameters(schema: unknown, job: Extravars, overrides: Extravars): Extravars {
  const merged: Extravars = { ...parameterDefaults(schema), ...job, ...overrides };
  for (const name of secretParameterNames(schema)) delete merged[name];
  return merged;
}

export function missingRequiredParameters(schema: unknown, values: Extravars): VarSpec[] {
  return parseVarsSchema(schema).filter((spec) => !isSecretParameter(spec) && spec.required && !Object.hasOwn(values, spec.name));
}

export function parameterErrors(schema: unknown, values: Extravars, requireAll = true): Record<string, string> {
  const specs = parseVarsSchema(schema);
  const errors: Record<string, string> = Object.create(null) as Record<string, string>;
  const declared = new Set(specs.map((spec) => spec.name));
  // Undeclared input is still allowed for jobs without a declared schema.
  if (isObject(schema) && isObject(schema.properties)) {
    for (const key of Object.keys(values)) if (!declared.has(key)) errors[key] = "undeclared";
  }
  for (const spec of specs) {
    if (isSecretParameter(spec)) continue;
    if (!Object.hasOwn(values, spec.name)) {
      if (requireAll && spec.required) errors[spec.name] = "required";
      continue;
    }
    const value = values[spec.name];
    const valid = value === null && spec.nullable ? true : spec.type === "array" ? Array.isArray(value)
      : spec.type === "object" ? isObject(value)
      : spec.type === "integer" ? typeof value === "number" && Number.isInteger(value)
      : spec.type === "null" ? value === null
      : typeof value === spec.type;
    if (!valid) errors[spec.name] = "type";
    else if (spec.choices && !spec.choices.some((choice) => JSON.stringify(choice) === JSON.stringify(value))) errors[spec.name] = "choice";
    else if (typeof value === "number" && ((typeof spec.schema.minimum === "number" && value < spec.schema.minimum) || (typeof spec.schema.maximum === "number" && value > spec.schema.maximum))) errors[spec.name] = "range";
    else if (typeof value === "string" && typeof spec.schema.minLength === "number" && value.length < spec.schema.minLength) errors[spec.name] = "length";
  }
  return errors;
}
