import { useTranslation } from "react-i18next";
import type { InventoryEntityKey, TemplateVariable } from "@/shared/api/types";
import EntityPicker, { type EntityPickerOption } from "@/shared/forms/EntityPicker";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FieldLabel from "@/shared/ui-shared/FieldLabel";
import FormSelect from "@/shared/ui-shared/FormSelect";
import {
  formatTemplateValue,
  getTemplateVariableOptions,
  isRequiredTemplateVariableWithoutDefault,
  parseTemplateObjectRows,
  TEMPLATE_REFERENCE_ENTITIES,
  type TemplateReferenceResource,
} from "./templateSchema";

interface OptionState {
  options: EntityPickerOption[];
  loading: boolean;
  error: boolean;
  retry: () => void;
}

interface Props {
  variable: TemplateVariable;
  fields: TemplateVariable[];
  value: unknown;
  onChange: (value: string) => void;
  errors: Record<string, string>;
  optionsByEntity: Map<InventoryEntityKey, OptionState>;
  onRequestCreate?: (entity: InventoryEntityKey, rowIndex: number, fieldKey: string) => void;
}

export default function TemplateRunDialogObjectFields({ variable, fields, value, onChange, errors, optionsByEntity, onRequestCreate }: Props) {
  const { t } = useTranslation();
  const parsed = parseTemplateObjectRows(value, fields);
  const orderedFields = [...fields].sort((left, right) =>
    Number(isRequiredTemplateVariableWithoutDefault(right)) - Number(isRequiredTemplateVariableWithoutDefault(left)),
  );
  const update = (index: number, key: string, next: string) => {
    onChange(JSON.stringify(parsed.rows.map((row, rowIndex) => rowIndex === index ? { ...row, [key]: next } : row)));
  };
  if (!fields.length) return <p role="alert" className="text-xs text-status-negative">{t("inventory.templates.run.invalidFields")}</p>;
  if (parsed.error) return <div className="grid gap-2">
    <p role="alert" className="text-xs text-status-negative">{t("inventory.templates.run.invalidCollection")}</p>
    <Textarea id={`run-var-${variable.key}`} aria-label={variable.label} value={formatTemplateValue(value)} onChange={(event) => onChange(event.target.value)} />
  </div>;

  return <div className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-4" role="group" aria-label={variable.label}>
    {parsed.rows.map((row, index) => <fieldset key={index} className="grid min-w-0 grid-cols-[minmax(0,1fr)] gap-2 rounded-md border border-border p-4">
      <legend className="max-w-full whitespace-normal break-words px-1 text-sm font-medium">{t("inventory.templates.run.objectRow", { label: variable.label, number: index + 1 })}</legend>
      {orderedFields.map((field) => {
        const key = `${variable.key}.${index}.${field.key}`;
        const id = `run-var-${key}`;
        const label = t("inventory.templates.run.objectRow", { label: field.label, number: index + 1 });
        const options = getTemplateVariableOptions(field);
        const fieldValue = formatTemplateValue(row[field.key]);
        const accessibility = {
          "aria-label": label,
          "aria-invalid": Boolean(errors[key]),
          "aria-describedby": [field.description ? `run-help-${key}` : "", errors[key] ? `run-error-${key}` : ""].filter(Boolean).join(" ") || undefined,
        };
        const entity = field.value_type === "resource_ref" ? TEMPLATE_REFERENCE_ENTITIES[field.validation_rules?.resource as TemplateReferenceResource] : null;
        const state = entity ? optionsByEntity.get(entity) : undefined;
        const retained = fieldValue && !state?.options.some((option) => option.id === fieldValue) ? [{ id: fieldValue, name: fieldValue }] : [];
        return <div key={field.key} id={`run-field-${key}`} className="grid gap-1.5">
          <FieldLabel htmlFor={id} required={field.required}>{field.label}</FieldLabel>
          {entity ? <>
            <EntityPicker {...accessibility} id={id} label={label} entity={entity} value={fieldValue} onChange={(next) => update(index, field.key, next)} options={[...retained, ...(state?.options ?? [])]} loading={state?.loading} onRequestCreate={onRequestCreate ? (requestedEntity) => onRequestCreate(requestedEntity, index, field.key) : undefined} />
            {state?.error && <div className="flex flex-wrap items-center gap-2"><p role="alert" className="text-xs text-status-negative">{t("inventory.templates.run.optionsError")}</p><Button type="button" variant="outline" size="sm" onClick={state.retry}>{t("inventory.templates.run.retry")}</Button></div>}
          </> : options.length || field.value_type === "boolean" ? <FormSelect {...accessibility} id={id} value={fieldValue} onValueChange={(next) => update(index, field.key, next)} options={options.length ? options.map((option) => ({ value: option, label: option })) : [{ value: "true", label: t("yes") }, { value: "false", label: t("no") }]} />
            : field.value_type === "json" ? <Textarea {...accessibility} id={id} value={fieldValue} onChange={(event) => update(index, field.key, event.target.value)} />
              : <Input {...accessibility} id={id} type={field.validation_rules?.format === "email" ? "email" : ["number", "integer"].includes(field.value_type) ? "number" : "text"} value={fieldValue} onChange={(event) => update(index, field.key, event.target.value)} />}
          {field.description && <p id={`run-help-${key}`} className="text-xs text-muted-foreground">{field.description}</p>}
          {errors[key] && <p id={`run-error-${key}`} role="alert" className="text-xs text-status-negative">{t("inventory.templates.run.rowError", { number: index + 1, message: errors[key] })}</p>}
        </div>;
      })}
      <Button type="button" variant="outline" size="sm" className="justify-self-start" aria-label={t("inventory.templates.run.removeObjectRow", { label: variable.label, number: index + 1 })} onClick={() => onChange(JSON.stringify(parsed.rows.filter((_, rowIndex) => rowIndex !== index)))}>{t("inventory.templates.run.removeRow")}</Button>
    </fieldset>)}
    <Button type="button" variant="outline" size="sm" className="justify-self-start" onClick={() => onChange(JSON.stringify([...parsed.rows, {}]))}>{t("inventory.templates.run.addObjectRow", { label: variable.label })}</Button>
  </div>;
}
