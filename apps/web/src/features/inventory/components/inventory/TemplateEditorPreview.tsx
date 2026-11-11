import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { useQueries } from "@tanstack/react-query";
import { Boxes } from "lucide-react";
import FormSection from "@/shared/ui-shared/FormSection";
import TemplatePreviewRows from "./TemplatePreviewRows";
import { buildTemplatePreview, buildTemplateVariableValues, formatTemplateValue, isTemplateSelection, templateReferenceFor, type InventoryTemplate } from "./templateTypes";
import { getTemplateVariableFields } from "./templateSchema";
import { inventoryResourceOptions, inventoryResourceOptionsQuery } from "@/features/inventory/resourceOptions";
import { resourceRefDefaultFor, resolvedReferenceDefault } from "./templateReferenceDefaults";

export default function TemplateEditorPreview({ template }: { template: InventoryTemplate }) {
  const { t } = useTranslation();
  const references = useMemo(() => template.variables.map(variable => ({ variable, reference: templateReferenceFor(template, variable) })).filter(item => item.reference !== null), [template]);
  const entities = useMemo(() => [...new Set(references.map(item => item.reference!.entity))], [references]);
  const queries = useQueries({ queries: entities.map(entity => ({ ...inventoryResourceOptionsQuery(entity), retry: false })) });
  const preview = useMemo(() => {
    const raw: Record<string, unknown> = Object.fromEntries(template.variables.map(variable => [variable.key, variable.value_type !== "json" && isTemplateSelection(variable.default_value) ? variable.default_value : formatTemplateValue(variable.default_value)]));
    const unavailable = new Set<string>();
    const pending = new Set<string>();
    for (const { variable, reference } of references) {
      if (!reference) continue;
      const defaultValue = variable.default_value;
      const hasDefault = defaultValue !== undefined && defaultValue !== null && (typeof defaultValue !== "string" || defaultValue.trim() !== "");
      if (!variable.required && !hasDefault && !resourceRefDefaultFor(variable)) continue;
      if (reference.value === "name" && typeof raw[variable.key] === "string" && (raw[variable.key] as string).trim()) continue;
      const query = queries[entities.indexOf(reference.entity)];
      if (!query?.isSuccess) { (query?.isError ? unavailable : pending).add(variable.key); continue; }
      const options = inventoryResourceOptions(reference.entity, query.data.items);
      const value = resolvedReferenceDefault(variable, reference, raw[variable.key], options);
      raw[variable.key] = value;
      if (isTemplateSelection(value) && (value.resource !== reference.resource || !options.some(option => option.id === value.id))) unavailable.add(variable.key);
      if (value === "" && (variable.default_value || resourceRefDefaultFor(variable))) unavailable.add(variable.key);
    }
    const parsed = buildTemplateVariableValues(template.variables, raw);
    for (const key of unavailable) parsed.errors[key] = "invalid_reference";
    for (const key of pending) delete parsed.errors[key];
    const errorKeys = Object.keys(parsed.errors);
    const missing = template.variables.filter(variable => errorKeys.some(key => key === variable.key || key.startsWith(`${variable.key}.`)));
    const missingLabels = errorKeys.map(key => {
      const variable = missing.find(candidate => key === candidate.key) ?? missing.find(candidate => key.startsWith(`${candidate.key}.`));
      if (!variable) return key;
      const label = variable.label || variable.key;
      if (key === variable.key) return label;
      const [row, fieldKey] = key.slice(variable.key.length + 1).split(".");
      if (!fieldKey || !Number.isInteger(Number(row)) || Number(row) < 0) return label;
      const field = getTemplateVariableFields(variable)?.find(candidate => candidate.key === fieldKey);
      return t("inventory.templates.editor.previewMissingRowField", { variable: label, row: Number(row) + 1, field: field?.label || fieldKey });
    });
    const values = { ...parsed.values };
    for (const variable of missing) if (variable.value_type !== "json") values[variable.key] = `{{${variable.key}}}`;
    for (const key of pending) values[key] = `{{${key}}}`;
    return { rows: buildTemplatePreview(template, values), missingLabels, loading: pending.size > 0 };
  }, [template, t, references, entities, queries]);

  return <FormSection title={t("inventory.templates.editor.previewTitle")}
    description={t("inventory.templates.editor.previewSubtitle")} contentClassName="flex flex-col gap-4">
    {preview.loading && <p role="status" className="text-sm text-muted-foreground">{t("loading")}</p>}
    {preview.missingLabels.length > 0 && <p role="status" className="rounded-md border border-border bg-background p-3 text-sm text-muted-foreground">
      {t("inventory.templates.editor.previewMissingInputs", { fields: preview.missingLabels.join(", ") })}
    </p>}
    {preview.rows.length ? <TemplatePreviewRows rows={preview.rows} /> : <div className="flex flex-col items-center gap-2 py-6 text-center text-muted-foreground">
      <Boxes className="size-8" aria-hidden />
      <p className="text-sm">{t(`inventory.templates.editor.${template.steps.length ? "previewNoRows" : "previewEmpty"}`)}</p>
    </div>}
  </FormSection>;
}
