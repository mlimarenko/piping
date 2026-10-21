import { useTranslation } from "react-i18next";
import { Plus, Trash2 } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Switch } from "@/shared/ui-primitives/switch";
import { cn } from "@/shared/lib/utils";
import { FormField, ToggleField } from "./PlaybookFormFields";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { VAR_TYPES, type VarSpec, type VarType } from "@/features/playbooks/varsSchema";

/**
 * Repeatable authoring list for a playbook's typed launch variables. Each row
 * declares one variable `{name, type, required, default, label?, choices?}`;
 * the parent serializes the rows into the `vars_schema` JSON Schema object via
 * `serializeVarsSchema`. The same rows drive the launch-form generator, so this
 * is purely an authoring surface — no submission logic lives here.
 */
function VarsSchemaEditor({
  rows,
  onChange,
  readOnly = false,
}: {
  rows: VarSpec[];
  onChange: (rows: VarSpec[]) => void;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();

  const update = (index: number, patch: Partial<VarSpec>) => {
    if (readOnly) return;
    onChange(rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  };

  const remove = (index: number) => {
    if (readOnly) return;
    onChange(rows.filter((_, i) => i !== index));
  };

  const add = () => {
    if (readOnly) return;
    onChange([
      ...rows,
      { name: "", type: "string", required: false, default: null },
    ]);
  };

  const changeType = (index: number, type: VarType) => {
    const row = rows[index];
    const patch: Partial<VarSpec> = { type, default: null };
    // Drop choice scaffolding when leaving the choice type; seed an empty list
    // when entering it.
    patch.choices = type === "choice" ? (row?.choices ?? []) : undefined;
    update(index, patch);
  };

  return (
    <div className="space-y-2">
      {rows.length === 0 ? (
        <p className="rounded-md border border-dashed border-border bg-background px-3 py-3 text-sm text-muted-foreground">
          {t("playbooks.page.varsSchema.empty")}
        </p>
      ) : (
        <div className="space-y-2">
          {rows.map((row, index) => (
            <div
              key={index}
              className="space-y-2 rounded-md border border-border bg-background p-3"
            >
              <div className="grid gap-2 sm:grid-cols-12">
                <FormField
                  label={t("playbooks.page.varsSchema.varName")}
                  className="sm:col-span-4"
                  labelClassName="text-sm font-medium text-foreground"
                >
                  <Input
                    className="h-9 font-mono text-sm"
                    value={row.name}
                    disabled={readOnly}
                    placeholder="target_path"
                    onChange={(e) => update(index, { name: e.target.value })}
                  />
                </FormField>
                <FormSelect
                  label={t("playbooks.page.varsSchema.varType")}
                  value={row.type}
                  disabled={readOnly}
                  onValueChange={(value) => changeType(index, value)}
                  className="sm:col-span-3"
                  labelClassName="text-sm font-medium text-foreground"
                  triggerClassName="h-9"
                  options={VAR_TYPES.map((type) => ({
                    value: type,
                    label: t(`playbooks.page.varsSchema.types.${type}`),
                  }))}
                />
                <FormField
                  label={t("playbooks.page.varsSchema.varLabel")}
                  className="sm:col-span-4"
                  labelClassName="text-sm font-medium text-foreground"
                >
                  <Input
                    className="h-9 text-sm"
                    value={row.label ?? ""}
                    disabled={readOnly}
                    placeholder={row.name || t("playbooks.page.varsSchema.varLabelPlaceholder")}
                    onChange={(e) =>
                      update(index, { label: e.target.value || undefined })
                    }
                  />
                </FormField>
                <div className="flex items-end justify-end sm:col-span-1">
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="size-9 text-muted-foreground hover:text-destructive"
                    disabled={readOnly}
                    aria-label={t("playbooks.page.varsSchema.removeVar")}
                    onClick={() => remove(index)}
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                  </Button>
                </div>
              </div>

              {row.type === "choice" ? (
                <FormField
                  label={t("playbooks.page.varsSchema.choices")}
                  labelClassName="text-sm font-medium text-foreground"
                >
                  <Input
                    className="h-9 text-sm"
                    value={(row.choices ?? []).join(", ")}
                    disabled={readOnly}
                    placeholder={t("playbooks.page.varsSchema.choicesPlaceholder")}
                    onChange={(e) =>
                      update(index, {
                        choices: e.target.value
                          .split(",")
                          .map((c) => c.trim())
                          .filter(Boolean),
                      })
                    }
                  />
                </FormField>
              ) : null}

              <FormField
                label={t("playbooks.page.varsSchema.varDescription")}
                labelClassName="text-sm font-medium text-foreground"
              >
                <Input
                  className="h-9 text-sm"
                  value={row.description ?? ""}
                  disabled={readOnly}
                  placeholder={t("playbooks.page.varsSchema.varDescriptionPlaceholder")}
                  onChange={(e) =>
                    update(index, {
                      description: e.target.value || undefined,
                    })
                  }
                />
              </FormField>

              <div className="grid gap-2 sm:grid-cols-12">
                <FormField
                  label={t("playbooks.page.varsSchema.varDefault")}
                  className="sm:col-span-8"
                  labelClassName="text-sm font-medium text-foreground"
                >
                  {row.type === "boolean" ? (
                    <div className="flex h-9 items-center gap-2">
                      <Switch
                        aria-label={t("playbooks.page.varsSchema.varDefault")}
                        checked={row.default === true}
                        disabled={readOnly}
                        onCheckedChange={(checked) =>
                          update(index, { default: checked })
                        }
                      />
                      <span className="text-xs text-muted-foreground">
                        {row.default === true
                          ? t("playbooks.page.varsSchema.boolTrue")
                          : t("playbooks.page.varsSchema.boolFalse")}
                        </span>
                      </div>
                  ) : row.type === "choice" ? (
                    <SearchableSelect
                      value={typeof row.default === "string" ? row.default : ""}
                      disabled={readOnly || (row.choices ?? []).length === 0}
                      onValueChange={(value) => update(index, { default: value })}
                      placeholder={t("playbooks.page.varsSchema.noDefault")}
                      searchPlaceholder={t("search")}
                      triggerClassName="h-9"
                      items={(row.choices ?? []).map((choice) => ({
                        id: choice,
                        label: choice,
                      }))}
                    />
                  ) : (
                    <Input
                      className={cn("h-9 text-sm", row.type === "number" && "font-mono")}
                      inputMode={row.type === "number" ? "numeric" : undefined}
                      value={row.default === null ? "" : String(row.default)}
                      disabled={readOnly}
                      placeholder={t("playbooks.page.varsSchema.noDefault")}
                      onChange={(e) => {
                        const raw = e.target.value;
                        if (raw === "") {
                          update(index, { default: null });
                        } else if (row.type === "number") {
                          const num = Number(raw);
                          update(index, {
                            default: Number.isFinite(num) ? num : raw,
                          });
                        } else {
                          update(index, { default: raw });
                        }
                      }}
                    />
                  )}
                </FormField>
                <div className="sm:col-span-4">
                  <ToggleField label={t("playbooks.page.varsSchema.required")} checked={row.required}
                    disabled={readOnly} onCheckedChange={(checked) => update(index, { required: checked })} />
                </div>
              </div>
            </div>
          ))}
        </div>
      )}

      {!readOnly ? (
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="gap-2"
          onClick={add}
        >
          <Plus className="h-3.5 w-3.5" />
          {t("playbooks.page.varsSchema.addVar")}
        </Button>
      ) : null}
    </div>
  );
}

export default VarsSchemaEditor;
