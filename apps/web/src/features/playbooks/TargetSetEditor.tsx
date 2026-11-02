import { useTranslation } from "react-i18next";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import type { ConnectionTarget } from "@/shared/api/types";
import type { SelectorModel } from "./selectorModel";
import type { SelectedHost } from "./hostSelection";
import type { TargetSetMode } from "./playbookFormModels";
import { FormField } from "./PlaybookFormFields";
import { RadioGroup, RadioGroupItem } from "@/shared/ui-primitives/radio-group";
import TargetSetSelectorBuilder from "./TargetSetSelectorBuilder";
import HostMultiSelectPicker from "./HostMultiSelectPicker";

export function TargetSetEditor({
  editing,
  name,
  onNameChange,
  mode,
  onModeChange,
  selectorModel,
  onSelectorModelChange,
  staticHosts,
  onStaticHostsChange,
  connectionTargets,
  connectionTargetsPending,
  onBack,
  onSubmit,
  submitting,
  dirty = false,
}: Readonly<{
  editing: boolean;
  name: string;
  onNameChange: (value: string) => void;
  mode: TargetSetMode;
  onModeChange: (mode: TargetSetMode) => void;
  selectorModel: SelectorModel;
  onSelectorModelChange: (model: SelectorModel) => void;
  staticHosts: SelectedHost[];
  onStaticHostsChange: (hosts: SelectedHost[]) => void;
  connectionTargets: ConnectionTarget[];
  connectionTargetsPending: boolean;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
}>) {
  const { t } = useTranslation();
  return (
    <EntityEditorPage scope="routed"
      title={
        editing
          ? t("playbooks.page.dialogs.targetSet.editTitle")
          : t("playbooks.page.dialogs.targetSet.title")
      }
      description={t("playbooks.page.dialogs.targetSet.description")}
      onBack={() => { if (!submitting) onBack(); }}
      backLabel={t("back")}
      onSave={onSubmit}
      saveLabel={editing ? t("save") : t("create")}
      saving={submitting}
      onCancel={onBack}
      cancelLabel={t("cancel")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
    >
      <fieldset disabled={submitting} inert={submitting} className="flex min-w-0 flex-col gap-4">
        <FormField label={t("name")}>
           <Input value={name} onChange={(event) => { if (!submitting) onNameChange(event.target.value); }} />
        </FormField>
        <RadioGroup value={mode} disabled={submitting} onValueChange={(value) => {
          if (!submitting && (value === "dynamic" || value === "static")) onModeChange(value);
        }} className="gap-4 sm:grid-cols-2">
          {(["dynamic", "static"] as const).map((value) => (
            <label key={value} className="flex items-start gap-2 text-sm">
              <RadioGroupItem value={value} className="mt-0.5" aria-label={t(`playbooks.page.targetSetMode.${value}`)} />
              <span className="flex flex-col gap-1">
                <span className="font-medium">{t(`playbooks.page.targetSetMode.${value}`)}</span>
                <span className="text-xs text-muted-foreground">{t(`playbooks.page.dialogs.targetSet.${value}Hint`)}</span>
              </span>
            </label>
          ))}
        </RadioGroup>
        {mode === "dynamic" ? (
          <TargetSetSelectorBuilder
            model={selectorModel}
            onChange={(next) => { if (!submitting) onSelectorModelChange(next); }}
            targets={connectionTargets}
          />
        ) : (
          <HostMultiSelectPicker
            targets={connectionTargets}
            selected={staticHosts}
            onChange={(next) => { if (!submitting) onStaticHostsChange(next); }}
            loading={connectionTargetsPending}
          />
        )}
      </fieldset>
    </EntityEditorPage>
  );
}
