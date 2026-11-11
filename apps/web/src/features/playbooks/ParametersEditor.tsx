import { useEffect, useId, useState } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { effectiveParameters, formatParameterValues, isSecretParameter, parameterErrors, parseParameterValues, parseVarsSchema, type Extravars, type JsonValue } from "./varsSchema";

interface Props {
  schema: unknown;
  value: string;
  onChange: (value: string) => void;
  onValidityChange: (valid: boolean) => void;
  inherited?: Extravars;
  fields?: readonly string[];
  requireAll?: boolean;
  disabled?: boolean;
}

export default function ParametersEditor({ schema, value, onChange, onValidityChange, inherited = {}, fields, requireAll = true, disabled = false }: Readonly<Props>) {
  const { t } = useTranslation();
  const id = useId();
  const [mode, setMode] = useState<"form" | "json" | "yaml">("form");
  const [draft, setDraft] = useState<string | null>(null);
  const [rawInvalid, setRawInvalid] = useState(false);
  const [fieldDrafts, setFieldDrafts] = useState<Record<string, string>>({});
  let values: Extravars = {};
  let invalid = false;
  try { values = parseParameterValues(value); } catch { invalid = true; }
  const effective = effectiveParameters(schema, inherited, values);
  const errors = invalid ? {} : parameterErrors(schema, effective, requireAll);
  const valid = !invalid && !rawInvalid && Object.keys(fieldDrafts).length === 0 && Object.keys(errors).length === 0;
  useEffect(() => { onValidityChange(valid); }, [onValidityChange, valid]);
  const allSpecs = parseVarsSchema(schema).filter((spec) => !fields || fields.includes(spec.name));
  const secretSpecs = allSpecs.filter(isSecretParameter);
  const specs = allSpecs.filter((spec) => !isSecretParameter(spec));
  const change = (name: string, next: JsonValue | undefined) => {
    const updated = { ...values };
    if (next === undefined) delete updated[name]; else Object.defineProperty(updated, name, { value: next, enumerable: true, writable: true, configurable: true });
    setDraft(null);
    setFieldDrafts((previous) => { const nextDrafts = { ...previous }; delete nextDrafts[name]; return nextDrafts; });
    onChange(JSON.stringify(updated, null, 2));
  };
  const raw = mode === "form" ? "" : draft ?? (invalid ? value : formatParameterValues(effective, mode));
  return <section className="flex min-w-0 flex-col gap-4" aria-label={t("playbooks.parameters.title", { defaultValue: "Parameters" })}>
    <div className="flex flex-wrap items-center gap-2">
      <h3 className="mr-auto text-sm font-medium">{t("playbooks.parameters.title", { defaultValue: "Parameters" })}</h3>
      {(["form", "yaml", "json"] as const).map((format) => <Button key={format} type="button" size="sm" variant={mode === format ? "secondary" : "outline"} aria-pressed={mode === format} disabled={disabled || ((invalid || rawInvalid || Object.keys(fieldDrafts).length > 0) && format !== mode)} onClick={() => { setMode(format); setDraft(null); }}>{format === "form" ? t("playbooks.parameters.fields", { defaultValue: "Fields" }) : format.toUpperCase()}</Button>)}
    </div>
    {mode === "form" ? <div className="flex min-w-0 flex-col gap-4">
      {!specs.length && <p className="text-sm text-muted-foreground">{t("playbooks.parameters.noFields", { defaultValue: "No parameters to fill in." })}</p>}
      {specs.map((spec, index) => {
        const inputId = `${id}-${index}`;
        const present = Object.hasOwn(effective, spec.name);
        const current = effective[spec.name];
        const own = Object.hasOwn(values, spec.name);
        const fieldError = Object.hasOwn(fieldDrafts, spec.name) ? "type" : errors[spec.name];
        return <div key={spec.name} className="flex min-w-0 flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2"><label htmlFor={inputId} className="text-sm font-medium">{spec.label}{spec.required ? " *" : ""}</label><code className="text-xs text-muted-foreground">{spec.name}</code>
            {own && <Button type="button" variant="ghost" size="sm" disabled={disabled || invalid} onClick={() => change(spec.name, undefined)}>{t("playbooks.parameters.reset", { defaultValue: "Use inherited value" })}</Button>}
            {spec.nullable && current !== null && <Button type="button" variant="ghost" size="sm" disabled={disabled || invalid} onClick={() => change(spec.name, null)}>null</Button>}
            {!present && spec.type === "string" && !spec.choices && <Button type="button" variant="ghost" size="sm" disabled={disabled || invalid} onClick={() => change(spec.name, "")}>{t("playbooks.parameters.emptyString", { defaultValue: "Set empty string" })}</Button>}
          </div>
          {spec.choices ? <FormSelect id={inputId} aria-label={spec.label} value={present ? JSON.stringify(current) : "__unset"} disabled={disabled || invalid} onValueChange={(next) => change(spec.name, next === "__unset" ? undefined : JSON.parse(next) as JsonValue)} options={[{ value: "__unset", label: t("playbooks.parameters.unset", { defaultValue: "Not set" }) }, ...spec.choices.map((choice) => ({ value: JSON.stringify(choice), label: typeof choice === "string" ? choice : JSON.stringify(choice) }))]} />
            : spec.type === "boolean" ? <FormSelect id={inputId} aria-label={spec.label} value={present ? String(current) : "__unset"} disabled={disabled || invalid} onValueChange={(next) => change(spec.name, next === "__unset" ? undefined : next === "null" ? null : next === "true")} options={[{ value: "__unset", label: t("playbooks.parameters.unset", { defaultValue: "Not set" }) }, { value: "true", label: "true" }, { value: "false", label: "false" }, ...(spec.nullable ? [{ value: "null", label: "null" }] : [])]} />
            : spec.type === "array" || spec.type === "object" || spec.type === "null" ? <Textarea id={inputId} aria-invalid={Boolean(fieldError)} value={fieldDrafts[spec.name] ?? (present ? JSON.stringify(current, null, 2) : "")} disabled={disabled || invalid} className="min-h-24 font-mono" placeholder={spec.type === "array" ? "[]" : "{}"} onChange={(event) => {
              const text = event.target.value;
              if (!text.trim()) { change(spec.name, undefined); return; }
              try { change(spec.name, JSON.parse(text) as JsonValue); } catch { setFieldDrafts((previous) => ({ ...previous, [spec.name]: text })); }
            }} />
            : <Input id={inputId} aria-invalid={Boolean(fieldError)} type={spec.type === "number" || spec.type === "integer" ? "number" : "text"} step={spec.type === "integer" ? 1 : "any"} value={current === null || current === undefined ? "" : String(current)} placeholder={current === null ? "null" : !present ? t("playbooks.parameters.unset", { defaultValue: "Not set" }) : undefined} disabled={disabled || invalid} onChange={(event) => change(spec.name, spec.type === "number" || spec.type === "integer" ? event.target.value === "" ? undefined : Number(event.target.value) : event.target.value)} />}
          {spec.description && <p className="text-sm text-muted-foreground">{spec.description}</p>}
          {fieldError && <p role="alert" className="text-sm text-destructive">{t(`playbooks.parameters.errors.${fieldError}`, { defaultValue: fieldError === "required" ? "Set a value." : "The value does not match the declared type or constraints." })}</p>}
        </div>;
      })}
      {secretSpecs.length > 0 && <div className="flex flex-col gap-1 rounded-md border border-dashed p-3">
        <p className="text-sm text-muted-foreground">{t("playbooks.parameters.secretsHint", { defaultValue: "These parameters are provided from the template's secret references through the run environment. They are references, not launch values." })}</p>
        {secretSpecs.map((spec) => <div key={spec.name} className="flex flex-wrap items-center gap-2"><span className="text-sm font-medium">{spec.label}</span><code className="text-xs text-muted-foreground">{spec.name}</code></div>)}
      </div>}
    </div> : <Textarea aria-label={`${t("playbooks.parameters.title", { defaultValue: "Parameters" })} ${mode.toUpperCase()}`} value={raw} disabled={disabled} className="min-h-48 font-mono" onChange={(event) => {
      const next = event.target.value;
      setDraft(next);
      try { onChange(JSON.stringify(parseParameterValues(next, mode), null, 2)); setRawInvalid(false); } catch { setRawInvalid(true); }
    }} />}
    {(invalid || rawInvalid) && <p role="alert" className="text-sm text-destructive">{t("playbooks.parameters.invalid", { defaultValue: "Enter a YAML or JSON object. Duplicate YAML keys and aliases are not allowed." })}</p>}
    {!invalid && Object.entries(errors).filter(([name]) => !specs.some((spec) => spec.name === name)).map(([name]) => <p key={name} role="alert" className="text-sm text-destructive">{t("playbooks.parameters.undeclared", { defaultValue: "Remove undeclared parameter: {{name}}", name })}</p>)}
  </section>;
}
