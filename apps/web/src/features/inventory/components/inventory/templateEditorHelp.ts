import { useTranslation } from "react-i18next";

/** A token offered by the insert-assist menu (`{{var}}` / `{{steps.<k>.id}}`). */
export interface TokenOption {
  /** The raw token to insert, e.g. `{{site_name}}` or `{{steps.site.id}}`. */
  token: string;
  /** Human label shown in the menu. */
  label: string;
  /** Optional secondary hint (e.g. variable type or producing step). */
  hint?: string;
}

/** Stable human-readable copy for the three conflict policies (for tooltips). */
export function useConflictPolicyHelp(): Record<string, string> {
  const { t } = useTranslation();
  return {
    fail: t("inventory.templates.editor.conflictHelp.fail"),
    skip: t("inventory.templates.editor.conflictHelp.skip"),
    update_selected_fields: t("inventory.templates.editor.conflictHelp.update"),
  };
}

/** Per-value-type explanatory copy (for the variable type tooltip). */
export function useValueTypeHelp(): Record<string, string> {
  const { t } = useTranslation();
  return {
    string: t("inventory.templates.editor.typeHelp.string"),
    integer: t("inventory.templates.editor.typeHelp.integer"),
    number: t("inventory.templates.editor.typeHelp.number"),
    boolean: t("inventory.templates.editor.typeHelp.boolean"),
    json: t("inventory.templates.editor.typeHelp.json"),
    resource_ref: t("inventory.templates.editor.typeHelp.resourceRef"),
  };
}
