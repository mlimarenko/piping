import { useTranslation } from "react-i18next";
import { Database, ServerCog } from "lucide-react";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import type { ConnectionTarget } from "@/shared/api/types";
import type { SelectorModel } from "./selectorModel";
import type { SelectedHost } from "./hostSelection";
import type { TargetSetMode } from "./playbookFormModels";
import { FormField } from "./PlaybookFormFields";
import { ModeTab } from "./PlaybookPanelUi";
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
        <div className="grid grid-cols-2 gap-1.5">
          <ModeTab
            active={mode === "dynamic"}
            icon={Database}
            label={t("playbooks.page.targetSetMode.dynamic")}
            hint={t("playbooks.page.dialogs.targetSet.dynamicHint")}
            onClick={() => { if (!submitting) onModeChange("dynamic"); }}
          />
          <ModeTab
            active={mode === "static"}
            icon={ServerCog}
            label={t("playbooks.page.targetSetMode.static")}
            hint={t("playbooks.page.dialogs.targetSet.staticHint")}
            onClick={() => { if (!submitting) onModeChange("static"); }}
          />
        </div>
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
