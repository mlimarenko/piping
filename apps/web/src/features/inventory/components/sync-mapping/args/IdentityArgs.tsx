import { useTranslation } from "react-i18next";
import { z } from "zod";
import FieldPathAutocomplete from "./FieldPathAutocomplete";
import type { ArgsFormProps } from "../OperatorArgsForm";

const schema = z.object({ field: z.string().min(1, "Field path is required") });

/**
 * identity(field) — pass field value unchanged.
 * Args: { field: string }
 * Shows autocomplete suggestions from the entity's known field paths (sourced
 * from entityFieldPaths prop supplied by ExpressionBuilder via OperatorArgsForm).
 */
const IdentityArgs = ({ args, onChange, disabled, entityFieldPaths }: ArgsFormProps) => {
  const { t } = useTranslation();
  const field = (args.field as string | undefined) ?? "";
  const result = schema.safeParse(args);
  const error = !result.success
    ? result.error.issues.find((i) => i.path[0] === "field")?.message
    : undefined;

  return (
    <div className="space-y-2">
      <FieldPathAutocomplete
        id="identity-field"
        label={t("sync.mapping.args.fieldPath", "Field path")}
        value={field}
        onChange={(next) => onChange({ ...args, field: next })}
        suggestions={entityFieldPaths}
        disabled={disabled}
        required
        error={error}
        placeholder="name"
      />
    </div>
  );
};

export default IdentityArgs;
