import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * coerce(s, type) — type cast field value.
 * Args: { field: string, to: "string" | "integer" | "float" | "boolean" | "uuid" | "datetime" }
 */

const TARGET_TYPES = ["string", "integer", "float", "boolean", "uuid", "datetime"] as const;

const schema = z.object({
  field: z.string().min(1, "Field path is required"),
  to: z.enum(TARGET_TYPES),
});

const CoerceArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const field = (args.field as string | undefined) ?? "";
  const to = (args.to as string | undefined) ?? "string";

  const result = schema.safeParse({ field, to });
  const err = (path: string) =>
    !result.success
      ? result.error.issues.find((i) => i.path[0] === path)?.message
      : undefined;

  return (
    <div className="space-y-2">
      <FormField
        htmlFor="coerce-field"
        label={t("sync.mapping.args.fieldPath", "Field path")}
        required
        error={err("field")}
      >
        <Input
          id="coerce-field"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="name"
          value={field}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, field: e.target.value })}
        />
      </FormField>
      <FormSelect
        id="coerce-to"
        label={t("sync.mapping.args.coerceTo", "To type")}
        value={to}
        disabled={disabled}
        onValueChange={(v) => onChange({ ...args, to: v })}
        triggerClassName="h-9 border-input/30"
        itemClassName="font-mono"
        options={TARGET_TYPES.map((type) => ({ value: type, label: type }))}
      />
    </div>
  );
};

export default CoerceArgs;
