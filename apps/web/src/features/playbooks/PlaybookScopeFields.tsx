import { useTranslation } from "react-i18next";
import { Input } from "@/shared/ui-primitives/input";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import EntitySelect from "@/shared/ui-shared/EntitySelect";
import FormField from "@/shared/ui-shared/FormField";
import FormSelect from "@/shared/ui-shared/FormSelect";
import type { PlaybookAssignmentScopeKind } from "@/features/playbooks/api";
import { FieldHelp } from "./FieldHelp";
import type { PlaybookScopeEntities } from "./usePlaybookScopeEntities";

const SCOPE_KINDS: PlaybookAssignmentScopeKind[] = [
  "global",
  "connection_target",
  "role",
  "site",
  "region",
  "tenant",
  "cluster",
  "platform",
];

interface PlaybookScopeFieldsProps {
  scopeKind: PlaybookAssignmentScopeKind;
  scopeId: string;
  onScopeKindChange: (next: PlaybookAssignmentScopeKind) => void;
  onScopeIdChange: (next: string) => void;
  entities: PlaybookScopeEntities;
}

/**
 * Scope-kind select + dependent entity picker for a playbook assignment.
 *
 * `global` carries no target (a disabled "Applies everywhere" hint); every
 * other kind requires a concrete entity, selected by id through the shared
 * {@link EntitySelect} so the stored `scope_id` is always an entity id and never
 * a typed UUID. Changing the kind resets the chosen id.
 */
const PlaybookScopeFields = ({
  scopeKind,
  scopeId,
  onScopeKindChange,
  onScopeIdChange,
  entities,
}: PlaybookScopeFieldsProps) => {
  const { t } = useTranslation();
  const options =
    scopeKind === "global" ? [] : entities.optionsByScope[scopeKind] ?? [];

  return (
    <TooltipProvider delayDuration={200}>
    <div className="grid gap-3 md:grid-cols-2">
      <FormSelect
        value={scopeKind}
        onValueChange={(value) => {
          onScopeKindChange(value);
          onScopeIdChange("");
        }}
        label={(
          <span className="flex items-center gap-1">
            {t("playbooks.page.fields.scopeKind")}
            <FieldHelp text={t("playbooks.page.help.scopeKind")} />
          </span>
        )}
        options={SCOPE_KINDS.map((kind) => ({
          value: kind,
          label: t(`playbooks.page.scope.${kind}`),
        }))}
      />
      <FormField
          label={(
            <span className="flex items-center gap-1">
              {t("playbooks.page.fields.scopeTarget")}
              <FieldHelp text={t("playbooks.page.help.scopeTarget")} />
            </span>
          )}
          required={scopeKind !== "global"}
        >
        {scopeKind === "global" ? (
          <Input
            className="h-9 text-sm"
            value={t("playbooks.page.placeholders.globalScope")}
            disabled
          />
        ) : (
          <EntitySelect
            items={options.map((option) => ({ id: option.id, label: option.name }))}
            value={scopeId}
            onValueChange={onScopeIdChange}
            triggerClassName="h-9 text-sm"
            placeholder={
              entities.isPending
                ? t("loading")
                : options.length === 0
                  ? t("playbooks.page.placeholders.noScopeTargets")
                  : t("playbooks.page.placeholders.scopeTarget")
            }
            searchPlaceholder={t("search")}
            aria-label={t("playbooks.page.fields.scopeTarget")}
          />
        )}
      </FormField>
    </div>
    </TooltipProvider>
  );
};

export default PlaybookScopeFields;
