import { useTranslation } from "react-i18next";
import { Input } from "@/shared/ui-primitives/input";
import { Info } from "lucide-react";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

/** upper(s) — uppercase string. Normalization operator. */

const UpperArgs = ({ args, onChange, disabled }: ArgsFormProps) => {
  const { t } = useTranslation();
  const field = (args.field as string | undefined) ?? "";

  return (
    <div className="space-y-2">
      <FormField
        htmlFor="upper-field"
        label={t("sync.mapping.args.fieldPath", "Field path")}
        required
      >
        <Input
          id="upper-field"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="name"
          value={field}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, field: e.target.value })}
        />
      </FormField>
      <div className="flex items-center gap-1.5 text-xs text-muted-foreground">
        <Info className="h-3 w-3 shrink-0" />
        {t("sync.mapping.args.normalizationHint", "Normalization-only — no automatic reverse. Idempotence tested.")}
      </div>
    </div>
  );
};

export default UpperArgs;
