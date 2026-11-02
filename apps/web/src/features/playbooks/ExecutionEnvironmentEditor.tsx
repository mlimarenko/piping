import { useTranslation } from "react-i18next";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";

import { FormField, ToggleField } from "./PlaybookFormFields";
import type { ExecutionEnvironmentForm } from "./playbookFormModels";

export function ExecutionEnvironmentEditor({
  form,
  onFormChange: changeDraft,
  onBack,
  onSubmit,
  submitting,
  dirty = false,
}: Readonly<{
  form: ExecutionEnvironmentForm;
  onFormChange: React.Dispatch<React.SetStateAction<ExecutionEnvironmentForm>>;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
}>) {
  const { t } = useTranslation();
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  return (
    <EntityEditorPage scope="routed"
      title={t("playbooks.page.dialogs.executionEnvironment.title")}
      description={t("playbooks.page.dialogs.executionEnvironment.description")}
      onBack={() => { if (!submitting) onBack(); }}
      backLabel={t("back")}
      onSave={onSubmit}
      saveLabel={t("create")}
      saving={submitting}
      saveDisabled={!form.name.trim() || !form.image.trim()}
      onCancel={onBack}
      cancelLabel={t("cancel")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
    >
      <fieldset disabled={submitting} inert={submitting} className="grid min-w-0 gap-4 md:grid-cols-2">
        <FormField label={t("name")}>
          <Input
            className="h-9 text-sm"
            value={form.name}
            onChange={(event) => onFormChange((prev) => ({ ...prev, name: event.target.value }))}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.image")}>
          <Input
            className="h-9 font-mono text-sm"
            value={form.image}
            onChange={(event) => onFormChange((prev) => ({ ...prev, image: event.target.value }))}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.description")} className="md:col-span-2">
          <Input
            className="h-9 text-sm"
            value={form.description}
            onChange={(event) =>
              onFormChange((prev) => ({ ...prev, description: event.target.value }))
            }
          />
        </FormField>
        <FormSelect
          value={form.pullPolicy}
          onValueChange={(pullPolicy) => onFormChange((prev) => ({ ...prev, pullPolicy }))}
          label={t("playbooks.page.executionEnvironments.pullPolicy")}
          options={["if_not_present", "always", "never"].map((policy) => ({
            value: policy,
            label: t(`playbooks.page.executionEnvironments.pullPolicies.${policy}`),
          }))}
        />
        <FormField label={t("playbooks.page.fields.runnerGroup")}>
          <Input
            className="h-9 text-sm"
            value={form.runnerGroup}
            onChange={(event) =>
              onFormChange((prev) => ({ ...prev, runnerGroup: event.target.value }))
            }
          />
        </FormField>
        <ToggleField
          label={t("enabled")}
          checked={form.enabled}
          disabled={submitting}
          onCheckedChange={(enabled) => onFormChange((prev) => ({ ...prev, enabled }))}
        />
      </fieldset>
    </EntityEditorPage>
  );
}
