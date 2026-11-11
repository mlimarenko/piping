import type { InventoryResourceOption } from "@/features/inventory/resourceOptions";
import { isTemplateReferenceId, isTemplateSelection, type InventoryTemplate, type TemplateReference, type TemplateSelection } from "./templateTypes";

const stringRule = (rules: Record<string, unknown>, keys: string[]): string | null => {
  for (const key of keys) {
    const value = rules[key];
    if (typeof value === "string" && value.trim()) return value.trim();
  }
  return null;
};

const stringArrayRule = (rules: Record<string, unknown>, keys: string[]): string[] => {
  for (const key of keys) {
    const value = rules[key];
    if (Array.isArray(value)) {
      return value
        .filter((item): item is string => typeof item === "string")
        .map((item) => item.trim())
        .filter(Boolean);
    }
  }
  return [];
};

export const resourceRefDefaultFor = (
  variable: InventoryTemplate["variables"][number],
): { id: string | null; names: string[] } | null => {
  const rules = variable.validation_rules;
  if (!rules || typeof rules !== "object" || Array.isArray(rules)) return null;
  const record = rules;
  const id = stringRule(record, ["default_id", "defaultId", "default_value_id", "defaultValueId"]);
  const names = [
    ...stringArrayRule(record, ["default_names", "defaultNames", "default_labels", "defaultLabels"]),
    stringRule(record, ["default_name", "defaultName", "default_label", "defaultLabel"]),
  ].filter((name): name is string => Boolean(name));
  return id || names.length > 0 ? { id, names } : null;
};

export const resolveResourceRefDefault = (
  options: InventoryResourceOption[],
  defaultConfig: { id: string | null; names: string[] },
): string | null => {
  // An explicit identity never falls through to a different object's name.
  if (defaultConfig.id) return defaultConfig.id;
  const matches = options.filter((option) =>
    defaultConfig.names.some((name) => option.name === name || option.slug === name),
  );
  return matches.length === 1 ? matches[0]?.id ?? null : null;
};

export function resolvedReferenceDefault(variable: InventoryTemplate["variables"][number], reference: TemplateReference, currentValue: unknown, options: InventoryResourceOption[]): unknown {
  if (isTemplateSelection(currentValue)) return currentValue;
  const value = typeof currentValue === "string" ? currentValue : "";
  if (reference.value === "name" && value) return currentValue;
  const config = resourceRefDefaultFor(variable);
  if ((currentValue === undefined || currentValue === null) && !config) return currentValue;
  const id = isTemplateReferenceId(value) ? value : resolveResourceRefDefault(options, config ?? { id: null, names: value ? [value] : [] });
  return id ? { resource: reference.resource, id } satisfies TemplateSelection : "";
}
