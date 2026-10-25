import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Boxes } from "lucide-react";
import FormSection from "@/shared/ui-shared/FormSection";
import TemplatePreviewRows from "./TemplatePreviewRows";
import { buildTemplatePreview, buildTemplateVariableValues, formatTemplateValue, type InventoryTemplate } from "./templateTypes";
import { getTemplateVariableFields } from "./templateSchema";

export default function TemplateEditorPreview({ template }: { template: InventoryTemplate }) {
  const { t } = useTranslation();
  const preview = useMemo(() => {
    const raw = Object.fromEntries(template.variables.map(variable => [variable.key, formatTemplateValue(variable.default_value)]));
    const parsed = buildTemplateVariableValues(template.variables, raw);
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
    return { rows: buildTemplatePreview(template, values), missingLabels };
  }, [template, t]);

  return <FormSection title={t("inventory.templates.editor.previewTitle")}
    description={t("inventory.templates.editor.previewSubtitle")} contentClassName="flex flex-col gap-4">
    {preview.missingLabels.length > 0 && <p role="status" className="rounded-md border border-border bg-background p-3 text-sm text-muted-foreground">
      {t("inventory.templates.editor.previewMissingInputs", { fields: preview.missingLabels.join(", ") })}
    </p>}
    {preview.rows.length ? <TemplatePreviewRows rows={preview.rows} /> : <div className="flex flex-col items-center gap-2 py-6 text-center text-muted-foreground">
      <Boxes className="size-8" aria-hidden />
      <p className="text-sm">{t(`inventory.templates.editor.${template.steps.length ? "previewNoRows" : "previewEmpty"}`)}</p>
    </div>}
  </FormSection>;
}
