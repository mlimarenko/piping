import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * split_index(s, sep, i) — extract segment by split.
 * Args: { field: string, separator: string, index: number }
 */

const schema = z.object({
  field: z.string().min(1, "Field path is required"),
  separator: z.string().min(1, "Separator is required"),
  index: z.number().int().min(0, "Index must be ≥ 0"),
});

const SplitIndexArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const field = (args.field as string | undefined) ?? "";
  const separator = (args.separator as string | undefined) ?? "-";
  const index = (args.index as number | undefined) ?? 0;

  const result = schema.safeParse({ field, separator, index });
  const err = (path: string) =>
    !result.success
      ? result.error.issues.find((i) => i.path[0] === path)?.message
      : undefined;

  return (
    <div className="space-y-2">
      <FormField
        htmlFor="split-field"
        label={t("sync.mapping.args.fieldPath", "Field path")}
        required
        error={err("field")}
      >
        <Input
          id="split-field"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="name"
          value={field}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, field: e.target.value })}
        />
      </FormField>
      <FormField
        htmlFor="split-sep"
        label={t("sync.mapping.args.separator", "Separator")}
        required
        error={err("separator")}
      >
        <Input
          id="split-sep"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder='"-"'
          value={separator}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, separator: e.target.value })}
        />
      </FormField>
      <FormField
        htmlFor="split-index"
        label={t("sync.mapping.args.index", "Index")}
        error={err("index")}
      >
        <Input
          id="split-index"
          type="number"
          min={0}
          className="h-9 text-sm font-mono border-input/30 mt-1"
          value={index}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, index: parseInt(e.target.value, 10) || 0 })}
        />
      </FormField>
    </div>
  );
};

export default SplitIndexArgs;
