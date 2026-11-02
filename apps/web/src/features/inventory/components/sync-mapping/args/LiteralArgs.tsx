import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * literal(value) — constant value.
 * Args: { value: string | number | boolean | null, value_type: "string" | "number" | "boolean" | "null" }
 */

const schema = z.object({
  value: z.union([z.string(), z.number(), z.boolean(), z.null()]),
  value_type: z.enum(["string", "number", "boolean", "null"]),
});

const VALUE_TYPES = ["string", "number", "boolean", "null"] as const;

const LiteralArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const valueType = (args.value_type as string | undefined) ?? "string";
  const value = (args.value as string | undefined) ?? "";

  const result = schema.safeParse(args);
  const error = !result.success
    ? result.error.issues.find((i) => i.path[0] === "value")?.message
    : undefined;

  return (
    <div className="space-y-2">
      <div className="flex gap-2">
        <FormField
          htmlFor="literal-value"
          label={t("sync.mapping.args.value", "Value")}
          error={error}
          className="flex-1"
        >
          <Input
            id="literal-value"
            className="h-9 text-sm font-mono border-input/30 mt-1"
            placeholder={valueType === "string" ? '"constant"' : "0"}
            value={value}
            disabled={disabled || valueType === "null"}
            onChange={(e) => onChange({ ...args, value: e.target.value })}
          />
        </FormField>
        <FormSelect
          id="literal-type"
          label={t("sync.mapping.args.type", "Type")}
          value={valueType}
          disabled={disabled}
          onValueChange={(v) => onChange({ ...args, value_type: v })}
          className="w-28"
          triggerClassName="h-9 border-input/30"
          options={VALUE_TYPES.map((type) => ({ value: type, label: type }))}
        />
      </div>
    </div>
  );
};

export default LiteralArgs;
