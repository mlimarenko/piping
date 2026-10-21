import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import { Button } from "@/shared/ui-primitives/button";
import { cn } from "@/shared/lib/utils";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/**
 * date_format(field, fmt) — format date field.
 * Args: { field: string, format: string }
 * Common formats as quick-pick chips.
 */

const QUICK_FORMATS = [
  { label: "ISO8601", value: "%Y-%m-%dT%H:%M:%SZ" },
  { label: "YYYY-MM-DD", value: "%Y-%m-%d" },
  { label: "Unix timestamp", value: "%s" },
] as const;

const schema = z.object({
  field: z.string().min(1, "Field path is required"),
  format: z.string().min(1, "Format string is required"),
});

const DateFormatArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const field = (args.field as string | undefined) ?? "";
  const format = (args.format as string | undefined) ?? "%Y-%m-%dT%H:%M:%SZ";

  const result = schema.safeParse({ field, format });
  const err = (path: string) =>
    !result.success
      ? result.error.issues.find((i) => i.path[0] === path)?.message
      : undefined;

  return (
    <div className="space-y-2">
      <FormField
        htmlFor="datefmt-field"
        label={t("sync.mapping.args.fieldPath", "Field path")}
        required
        error={err("field")}
      >
        <Input
          id="datefmt-field"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="created_at"
          value={field}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, field: e.target.value })}
        />
      </FormField>
      <FormField
        htmlFor="datefmt-format"
        label={t("sync.mapping.args.format", "Format")}
        required
        error={err("format")}
      >
        <Input
          id="datefmt-format"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="%Y-%m-%dT%H:%M:%SZ"
          value={format}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, format: e.target.value })}
        />
        {/* Quick-pick chips */}
        {!disabled && (
          <div className="flex gap-1 mt-1.5 flex-wrap">
            {QUICK_FORMATS.map((qf) => (
              <Button
                key={qf.value}
                variant="outline"
                size="sm"
                className={cn(
                  "h-5 text-sm px-1.5",
                  format === qf.value && "border-primary/50 text-primary",
                )}
                onClick={() => onChange({ ...args, format: qf.value })}
              >
                {qf.label}
              </Button>
            ))}
          </div>
        )}
      </FormField>
    </div>
  );
};

export default DateFormatArgs;
