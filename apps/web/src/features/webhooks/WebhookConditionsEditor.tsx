import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Code2, Plus, SlidersHorizontal, Trash2 } from "lucide-react";

import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import { Button } from "@/shared/ui-primitives/button";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { cn } from "@/shared/lib/utils";
import {
  CONDITION_OPS,
  emptyConditionRow,
  serializeConditions,
  type ConditionLogic,
  type ConditionOp,
  type ConditionRow,
} from "@/features/webhooks/webhookEvents";

interface WebhookConditionsEditorProps {
  logic: ConditionLogic;
  rows: ConditionRow[];
  onChange: (next: { logic: ConditionLogic; rows: ConditionRow[] }) => void;
}

type Mode = "structured" | "raw";

/**
 * NetBox-style ConditionSet editor: a flat list of `{ attr, op, value }` rows
 * joined by a top-level AND/OR, serializing to
 * `{ "and" | "or": [ { attr, op, value } ] }`. An empty list omits `conditions`
 * (no filter). A collapsible raw-JSON escape hatch round-trips with the
 * structured rows when valid.
 *
 * Renders inline; the row list is short and lives in the dialog's scroll
 * region. The op `Select` is a single small Radix dropdown (not a scrollable
 * list inside the dialog), which Radix positions correctly within a Dialog.
 */
const WebhookConditionsEditor = ({ logic, rows, onChange }: WebhookConditionsEditorProps) => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(rows.length > 0);
  const [mode, setMode] = useState<Mode>("structured");
  // Raw-mode buffer + parse error. Only touched while in raw mode.
  const [rawText, setRawText] = useState("");
  const [rawError, setRawError] = useState<string | null>(null);

  const setLogic = (next: ConditionLogic) => onChange({ logic: next, rows });

  const updateRow = (id: string, patch: Partial<ConditionRow>) => {
    onChange({ logic, rows: rows.map((r) => (r.id === id ? { ...r, ...patch } : r)) });
  };

  const addRow = () => onChange({ logic, rows: [...rows, emptyConditionRow()] });

  const removeRow = (id: string) => {
    onChange({ logic, rows: rows.filter((r) => r.id !== id) });
  };

  const opLabel = (op: ConditionOp): string => t(`webhooks.conditions.ops.${op}`);

  // Switching to raw mode seeds the textarea from the current structured rows.
  const enterRawMode = () => {
    const set = serializeConditions(logic, rows);
    setRawText(set ? JSON.stringify(set, null, 2) : "");
    setRawError(null);
    setMode("raw");
  };

  // Applying raw mode parses the JSON back into structured rows; on success it
  // switches back to the structured editor, on failure it surfaces the error
  // and stays in raw mode. All state changes happen in this handler, never in
  // an effect.
  const applyRawMode = () => {
    const text = rawText.trim();
    if (text === "") {
      onChange({ logic: "and", rows: [] });
      setRawError(null);
      setMode("structured");
      return;
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      setRawError(t("webhooks.conditions.rawInvalid"));
      return;
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      setRawError(t("webhooks.conditions.rawShapeInvalid"));
      return;
    }
    const obj = parsed as Record<string, unknown>;
    const nextLogic: ConditionLogic = Array.isArray(obj.or) ? "or" : "and";
    const list = obj[nextLogic];
    if (!Array.isArray(list)) {
      setRawError(t("webhooks.conditions.rawShapeInvalid"));
      return;
    }
    const nextRows: ConditionRow[] = [];
    for (const entry of list) {
      if (entry === null || typeof entry !== "object") continue;
      const leaf = entry as Record<string, unknown>;
      const attr = typeof leaf.attr === "string" ? leaf.attr : "";
      const op =
        typeof leaf.op === "string" && (CONDITION_OPS as readonly string[]).includes(leaf.op)
          ? (leaf.op as ConditionOp)
          : "eq";
      const value =
        typeof leaf.value === "string" ? leaf.value : JSON.stringify(leaf.value ?? "");
      nextRows.push({ ...emptyConditionRow(), attr, op, value });
    }
    onChange({ logic: nextLogic, rows: nextRows });
    setRawError(null);
    setMode("structured");
  };

  const completeCount = rows.filter((r) => r.attr.trim() !== "").length;

  return (
    <div className="rounded-lg border border-border/40 bg-card">
      <Button variant="ghost" size="sm"
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-expanded={open}
        className="h-auto min-h-9 w-full justify-between gap-2 whitespace-normal px-3 py-2 text-left"
      >
        <span className="flex items-center gap-1.5 text-sm font-medium text-foreground">
          <SlidersHorizontal className="h-3.5 w-3.5 text-muted-foreground" />
          {t("webhooks.conditions.title")}
          <span className="text-xs font-normal text-muted-foreground">
            {t("webhooks.conditions.optional")}
          </span>
        </span>
        <span className="text-xs tabular-nums text-muted-foreground">
          {completeCount > 0
            ? t("webhooks.conditions.activeCount", { count: completeCount })
            : t("webhooks.conditions.none")}
        </span>
      </Button>

      {open && (
        <div className="space-y-2.5 border-t border-border/40 px-3 py-3">
          <p className="text-xs leading-relaxed text-muted-foreground">
            {t("webhooks.conditions.help")}
          </p>

          {mode === "structured" ? (
            <>
              {rows.length > 1 && (
                <div className="flex items-center gap-1.5">
                  <span className="text-xs uppercase tracking-wide text-muted-foreground">
                    {t("webhooks.conditions.match")}
                  </span>
                  <div className="inline-flex overflow-hidden rounded-md border border-border/40">
                    {(["and", "or"] as const).map((opt) => (
                      <Button variant="ghost" size="sm"
                        key={opt}
                        type="button"
                        onClick={() => setLogic(opt)}
                        aria-pressed={logic === opt}
                        className={cn(
                          "h-9 px-3 text-sm font-medium transition-colors",
                          logic === opt
                            ? "bg-accent text-primary"
                            : "text-muted-foreground hover:text-foreground",
                        )}
                      >
                        {t(`webhooks.conditions.logic.${opt}`)}
                      </Button>
                    ))}
                  </div>
                </div>
              )}

              {rows.length === 0 ? (
                <p className="rounded-md border border-dashed border-border/40 px-3 py-3 text-center text-xs text-muted-foreground">
                  {t("webhooks.conditions.empty")}
                </p>
              ) : (
                <div className="space-y-1.5">
                  {rows.map((row) => (
                    <div key={row.id} className="grid min-w-0 grid-cols-[minmax(0,1fr)_auto] items-center gap-2 sm:grid-cols-[minmax(0,1fr)_112px_minmax(0,1fr)_auto]">
                      <Input
                        value={row.attr}
                        onChange={(e) => updateRow(row.id, { attr: e.target.value })}
                        placeholder={t("webhooks.conditions.attrPlaceholder")}
                        className="min-w-0 font-mono"
                        aria-label={t("webhooks.conditions.attrLabel")}
                      />
                      <FormSelect<ConditionOp>
                        value={row.op}
                        onValueChange={(op) => updateRow(row.id, { op })}
                        options={CONDITION_OPS.map((op) => ({ value: op, label: opLabel(op) }))}
                        triggerClassName="w-full"
                        aria-label={t("webhooks.conditions.opLabel")}
                      />
                      <Input
                        value={row.value}
                        onChange={(e) => updateRow(row.id, { value: e.target.value })}
                        placeholder={t("webhooks.conditions.valuePlaceholder")}
                        className="min-w-0 font-mono"
                        aria-label={t("webhooks.conditions.valueLabel")}
                      />
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 shrink-0 text-muted-foreground hover:text-status-negative"
                        onClick={() => removeRow(row.id)}
                        aria-label={t("webhooks.conditions.removeRow")}
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </Button>
                    </div>
                  ))}
                </div>
              )}

              <div className="flex items-center justify-between gap-2">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-9 gap-2 px-3 text-sm"
                  onClick={addRow}
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t("webhooks.conditions.addRow")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-9 gap-2 px-3 text-sm text-muted-foreground hover:text-foreground"
                  onClick={enterRawMode}
                >
                  <Code2 className="h-3.5 w-3.5" />
                  {t("webhooks.conditions.editRaw")}
                </Button>
              </div>
            </>
          ) : (
            <div className="space-y-2">
              <label htmlFor="webhook-conditions-raw" className="block text-sm font-medium text-foreground">
                {t("webhooks.conditions.rawLabel")}
              </label>
              <Textarea
                id="webhook-conditions-raw"
                value={rawText}
                onChange={(e) => {
                  setRawText(e.target.value);
                  if (rawError) setRawError(null);
                }}
                spellCheck={false}
                rows={8}
                placeholder={'{\n  "and": [\n    { "attr": "status", "op": "eq", "value": "active" }\n  ]\n}'}
                className={cn(
                  "font-mono text-sm leading-relaxed",
                  rawError ? "border-status-negative/60" : "border-input/40",
                )}
              />
               {rawError && <p role="alert" className="text-xs text-status-negative">{rawError}</p>}
              <div className="flex items-center justify-end gap-2">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-9 px-3 text-sm text-muted-foreground hover:text-foreground"
                  onClick={() => {
                    setRawError(null);
                    setMode("structured");
                  }}
                >
                  {t("cancel")}
                </Button>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className="h-9 px-3 text-sm"
                  onClick={applyRawMode}
                >
                  {t("webhooks.conditions.applyRaw")}
                </Button>
              </div>
            </div>
          )}
        </div>
      )}
    </div>
  );
};

export default WebhookConditionsEditor;
