import { useTranslation } from "react-i18next";
import { Plus, X, GripVertical } from "lucide-react";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import FormSelect from "@/shared/ui-shared/FormSelect";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * concat(a, b, ...) — join multiple fields or literals.
 * Args: { operands: Array<{ kind: "field" | "literal", value: string }> }
 * Minimum 2 operands enforced.
 * Follow-up: drag-and-drop reorder (currently up/down arrows as keyboard-accessible alternative)
 */

interface Operand {
  kind: "field" | "literal";
  value: string;
}

const schema = z.object({
  operands: z.array(z.object({ kind: z.enum(["field", "literal"]), value: z.string() })).min(2, "At least 2 operands required"),
});

const ConcatArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const operands: Operand[] = (args.operands as Operand[] | undefined) ?? [
    { kind: "field", value: "" },
    { kind: "field", value: "" },
  ];

  const result = schema.safeParse(args);
  const rootError =
    !result.success && result.error.issues.find((i) => i.path[0] === "operands" && i.path.length === 1)
      ? result.error.issues.find((i) => i.path[0] === "operands" && i.path.length === 1)!.message
      : undefined;

  const update = (newOps: Operand[]) => onChange({ ...args, operands: newOps });

  const addOperand = () => update([...operands, { kind: "field", value: "" }]);
  const removeOperand = (i: number) => update(operands.filter((_, idx) => idx !== i));
  const moveUp = (i: number) => {
    if (i === 0) return;
    const next = [...operands];
    const a = next[i - 1];
    const b = next[i];
    if (a === undefined || b === undefined) return;
    next[i - 1] = b;
    next[i] = a;
    update(next);
  };
  const moveDown = (i: number) => {
    if (i === operands.length - 1) return;
    const next = [...operands];
    const a = next[i];
    const b = next[i + 1];
    if (a === undefined || b === undefined) return;
    next[i] = b;
    next[i + 1] = a;
    update(next);
  };

  return (
    <div className="space-y-2">
      {operands.map((op, i) => (
        <div key={i} className="flex items-center gap-1.5">
          {/* Drag handle placeholder */}
          <GripVertical
            className="h-3.5 w-3.5 text-muted-foreground shrink-0 cursor-grab"
            aria-label={t("sync.mapping.args.reorderOperand", "Reorder operand {{n}}", { n: i + 1 })}
          />
          <FormSelect
            value={op.kind}
            disabled={disabled}
            onValueChange={(v) => {
              const next = [...operands];
              next[i] = { ...op, kind: v };
              update(next);
            }}
            className="w-20 shrink-0"
            triggerClassName="h-9 border-input/30"
            options={[
              { value: "field", label: "field" },
              { value: "literal", label: "literal" },
            ]}
          />
          <Input
            className="h-9 text-sm font-mono border-input/30 flex-1"
            placeholder={op.kind === "field" ? "name" : '"separator"'}
            value={op.value}
            disabled={disabled}
            onChange={(e) => {
              const next = [...operands];
              next[i] = { ...op, value: e.target.value };
              update(next);
            }}
          />
          {/* Up/down keyboard-accessible reorder */}
          <div className="flex flex-col shrink-0">
            <Button variant="ghost"
              type="button"
              className="text-muted-foreground hover:text-foreground disabled:opacity-20 leading-none text-sm"
              onClick={() => moveUp(i)}
              disabled={disabled || i === 0}
              aria-label={`Move operand ${i + 1} up`}
            >
              ▲
            </Button>
            <Button variant="ghost"
              type="button"
              className="text-muted-foreground hover:text-foreground disabled:opacity-20 leading-none text-sm"
              onClick={() => moveDown(i)}
              disabled={disabled || i === operands.length - 1}
              aria-label={`Move operand ${i + 1} down`}
            >
              ▼
            </Button>
          </div>
          <Button
            variant="ghost"
            size="icon"
            className="size-9 shrink-0"
            disabled={disabled || operands.length <= 2}
            onClick={() => removeOperand(i)}
            aria-label={`Remove operand ${i + 1}`}
          >
            <X className="h-3 w-3" />
          </Button>
        </div>
      ))}
      {rootError && (
        <p role="alert" className="text-xs text-destructive mt-0.5">{rootError}</p>
      )}
      {!disabled && (
        <Button
          variant="outline"
          size="sm"
          className="h-9 text-sm gap-1"
          onClick={addOperand}
        >
          <Plus className="h-3 w-3" />
          {t("sync.mapping.args.addOperand", "Add operand")}
        </Button>
      )}
    </div>
  );
};

export default ConcatArgs;
