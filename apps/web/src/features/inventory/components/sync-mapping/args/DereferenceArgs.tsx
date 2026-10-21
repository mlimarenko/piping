import { useTranslation } from "react-i18next";
import { z } from "zod";
import { Input } from "@/shared/ui-primitives/input";
import FieldPathAutocomplete from "./FieldPathAutocomplete";
import FormField from "@/shared/ui-shared/FormField";
import type { ArgsFormProps } from "../OperatorArgsForm";

const schema = z.object({
  fk_field: z.string().min(1, "FK field is required"),
  target_attr: z.string().min(1, "Target attribute is required"),
});

/**
 * dereference(fk_field, target_attr) — resolve FK to attribute.
 * Args: { fk_field: string, target_attr: string }
 * Autocomplete on fk_field from entity field paths.
 */
const DereferenceArgs = ({ args, onChange, disabled, entityFieldPaths }: ArgsFormProps) => {
  const { t } = useTranslation();
  const fkField = (args.fk_field as string | undefined) ?? "";
  const targetAttr = (args.target_attr as string | undefined) ?? "";
  const result = schema.safeParse({ fk_field: fkField, target_attr: targetAttr });
  const err = (path: string) =>
    !result.success
      ? result.error.issues.find((i) => i.path[0] === path)?.message
      : undefined;

  return (
    <div className="space-y-2">
      <FieldPathAutocomplete
        id="deref-fk"
        label={t("sync.mapping.args.fkField", "FK field")}
        value={fkField}
        onChange={(next) => onChange({ ...args, fk_field: next })}
        suggestions={entityFieldPaths}
        disabled={disabled}
        required
        error={err("fk_field")}
        placeholder="site"
      />

      <FormField
        htmlFor="deref-attr"
        label={t("sync.mapping.args.targetAttr", "Target attribute")}
        required
        error={err("target_attr")}
      >
        <Input
          id="deref-attr"
          className="h-9 text-sm font-mono border-input/30 mt-1"
          placeholder="name"
          value={targetAttr}
          disabled={disabled}
          onChange={(e) => onChange({ ...args, target_attr: e.target.value })}
        />
      </FormField>
    </div>
  );
};

export default DereferenceArgs;
