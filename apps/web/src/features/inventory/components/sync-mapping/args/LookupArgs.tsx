import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Plus, X } from "lucide-react";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * lookup(table, key) — key/value table substitution.
 * Args: { key_field: string, table: Array<{ from: string, to: string }> }
 * Includes "Paste CSV" shortcut: from,to lines.
 * Follow-up: duplicate "from" value validation
 */

interface LookupRow {
  from: string;
  to: string;
}

const schema = z.object({
  key_field: z.string().min(1, "Key field is required"),
  table: z.array(z.object({ from: z.string(), to: z.string() })),
});

const LookupArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const [csvMode, setCsvMode] = useState(false);
  const [csvText, setCsvText] = useState("");
  const [csvError, setCsvError] = useState<string | null>(null);

  const keyField = (args.key_field as string | undefined) ?? "";
  const table: LookupRow[] = (args.table as LookupRow[] | undefined) ?? [];

  const result = schema.safeParse({ key_field: keyField, table });
  const keyErr = !result.success
    ? result.error.issues.find((i) => i.path[0] === "key_field")?.message
    : undefined;

  const updateTable = (newTable: LookupRow[]) =>
    onChange({ ...args, table: newTable });

  const addRow = () => updateTable([...table, { from: "", to: "" }]);
  const removeRow = (i: number) => updateTable(table.filter((_, idx) => idx !== i));
  const updateRow = (i: number, patch: Partial<LookupRow>) => {
    const next = [...table];
    const current = next[i];
    if (!current) return;
    next[i] = { ...current, ...patch };
    updateTable(next);
  };

  const applyCsv = () => {
    try {
      const rows = csvText
        .split("\n")
        .map((line) => line.trim())
        .filter(Boolean)
        .map((line) => {
          const commaIdx = line.indexOf(",");
          if (commaIdx === -1) throw new Error(`Invalid line: "${line}"`);
          return { from: line.slice(0, commaIdx).trim(), to: line.slice(commaIdx + 1).trim() };
        });
      updateTable([...table, ...rows]);
      setCsvMode(false);
      setCsvText("");
      setCsvError(null);
    } catch (e) {
      setCsvError((e as Error).message);
    }
  };

  return (
    <div className="space-y-2">
      {/* Key field */}
      <FormField
        htmlFor="lookup-key"
        label={t("sync.mapping.args.keyField", "Key field")}
        required
        error={keyErr}
      >
        <Input
          id="lookup-key"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="status"
          value={keyField}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, key_field: e.target.value })}
        />
      </FormField>

      {/* Table header */}
      <div className="text-xs font-medium text-muted-foreground">
        {t("sync.mapping.args.lookupTable", "Lookup table")}
      </div>

      {/* Rows */}
      <div className="space-y-1">
        {table.map((row, i) => (
          <div key={i} className="flex items-center gap-1.5">
            <Input
              className="h-9 text-sm font-mono border-input/30 flex-1"
              placeholder={t("sync.mapping.args.from", "from")}
              value={row.from}
              disabled={disabled}
              onChange={(e) => updateRow(i, { from: e.target.value })}
            />
            <span className="text-muted-foreground text-xs shrink-0">→</span>
            <Input
              className="h-9 text-sm font-mono border-input/30 flex-1"
              placeholder={t("sync.mapping.args.to", "to")}
              value={row.to}
              disabled={disabled}
              onChange={(e) => updateRow(i, { to: e.target.value })}
            />
            <Button
              variant="ghost"
              size="icon"
              className="size-9 shrink-0"
              disabled={disabled}
              onClick={() => removeRow(i)}
            >
              <X className="h-3 w-3" />
            </Button>
          </div>
        ))}
      </div>

      {/* Actions */}
      {!disabled && (
        <div className="flex gap-2">
          <Button variant="outline" size="sm" className="h-9 text-sm gap-1" onClick={addRow}>
            <Plus className="h-3 w-3" />
            {t("sync.mapping.args.addRow", "Add row")}
          </Button>
          <Button
            variant="ghost"
            size="sm"
            className="h-9 text-sm"
            onClick={() => setCsvMode((v) => !v)}
          >
            {t("sync.mapping.args.pasteCsv", "Paste CSV")}
          </Button>
        </div>
      )}

      {/* CSV textarea */}
      {csvMode && (
        <div className="space-y-2">
          <Textarea
            className="font-mono text-xs h-28 border-input/30 resize-none"
            placeholder={"active,active\nplanned,planning\nstaged,staging"}
            value={csvText}
            onChange={(e) => setCsvText(e.target.value)}
          />
          {csvError && <p role="alert" className="text-xs text-destructive">{csvError}</p>}
          <div className="flex gap-2">
            <Button size="sm" className="h-9 text-sm" onClick={applyCsv}>
              {t("sync.mapping.args.applyCSV", "Apply")}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="h-9 text-sm"
              onClick={() => { setCsvMode(false); setCsvError(null); }}
            >
              {t("cancel", "Cancel")}
            </Button>
          </div>
        </div>
      )}
    </div>
  );
};

export default LookupArgs;
