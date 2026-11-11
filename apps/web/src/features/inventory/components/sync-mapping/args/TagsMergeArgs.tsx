import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import { Info } from "lucide-react";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * tags_merge(a, b) — union two tag arrays.
 * Args: { field_a: string, field_b: string }
 */

const schema = z.object({
  field_a: z.string().min(1, "Array A field is required"),
  field_b: z.string().min(1, "Array B field is required"),
});

const TagsMergeArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const fieldA = (args.field_a as string | undefined) ?? "";
  const fieldB = (args.field_b as string | undefined) ?? "";

  const result = schema.safeParse({ field_a: fieldA, field_b: fieldB });
  const err = (path: string) =>
    !result.success
      ? result.error.issues.find((i) => i.path[0] === path)?.message
      : undefined;

  return (
    <div className="space-y-2">
      <FormField
        htmlFor="tags-a"
        label={t("sync.mapping.args.arrayA", "Array A")}
        required
        error={err("field_a")}
      >
        <Input
          id="tags-a"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="tags"
          value={fieldA}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, field_a: e.target.value })}
        />
      </FormField>
      <FormField
        htmlFor="tags-b"
        label={t("sync.mapping.args.arrayB", "Array B")}
        required
        error={err("field_b")}
      >
        <Input
          id="tags-b"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="custom_tags"
          value={fieldB}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, field_b: e.target.value })}
        />
      </FormField>
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Info className="h-3 w-3 shrink-0" />
        {t("sync.mapping.args.normalizationHint", "Normalization-only — no automatic reverse. Idempotence tested.")}
      </div>
    </div>
  );
};

export default TagsMergeArgs;
