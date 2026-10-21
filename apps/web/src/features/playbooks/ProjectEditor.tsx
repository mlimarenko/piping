import { useTranslation } from "react-i18next";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type { PlaybookExecutionEnvironment } from "@/features/playbooks/api";

import { FormField } from "./PlaybookFormFields";
import type { ProjectForm } from "./playbookFormModels";

export function ProjectEditor({
  form,
  executionEnvironments,
  onFormChange: changeDraft,
  onBack,
  onSubmit,
  submitting,
  dirty = false,
}: Readonly<{
  form: ProjectForm;
  executionEnvironments: PlaybookExecutionEnvironment[];
  onFormChange: React.Dispatch<React.SetStateAction<ProjectForm>>;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
}>) {
  const { t } = useTranslation();
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  return (
    <EntityEditorPage scope="routed"
      title={t("playbooks.page.dialogs.project.title")}
      description={t("playbooks.page.dialogs.project.description")}
      onBack={() => { if (!submitting) onBack(); }}
      backLabel={t("back")}
      onSave={onSubmit}
      saveLabel={t("create")}
      saving={submitting}
      saveDisabled={!form.name.trim() || (form.scmType === "git" && !form.scmUrl.trim())}
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
        <FormSelect
          value={form.scmType}
          onValueChange={(scmType) => onFormChange((prev) => ({ ...prev, scmType }))}
          label={t("playbooks.page.projects.scmTypeLabel")}
          options={["manual", "git"].map((type) => ({
            value: type,
            label: t(`playbooks.page.projects.scmType.${type}`),
          }))}
        />
        <FormField label={t("playbooks.page.fields.description")} className="md:col-span-2">
          <Input
            className="h-9 text-sm"
            value={form.description}
            onChange={(event) =>
              onFormChange((prev) => ({ ...prev, description: event.target.value }))
            }
          />
        </FormField>
        {form.scmType === "git" ? (
          <>
            <FormField label={t("playbooks.page.projects.scmUrl")} className="md:col-span-2">
              <Input
                className="h-9 font-mono text-sm"
                value={form.scmUrl}
                placeholder="https://git.example/team/automation.git"
                onChange={(event) =>
                  onFormChange((prev) => ({ ...prev, scmUrl: event.target.value }))
                }
              />
            </FormField>
            <FormField label={t("playbooks.page.projects.branch")}>
              <Input
                className="h-9 text-sm"
                value={form.scmBranch}
                onChange={(event) =>
                  onFormChange((prev) => ({ ...prev, scmBranch: event.target.value }))
                }
              />
            </FormField>
            <FormField label={t("playbooks.page.projects.ref")}>
              <Input
                className="h-9 text-sm"
                value={form.scmRef}
                onChange={(event) =>
                  onFormChange((prev) => ({ ...prev, scmRef: event.target.value }))
                }
              />
            </FormField>
          </>
        ) : null}
        <FormField label={t("playbooks.page.fields.basePath")}>
          <Input
            className="h-9 font-mono text-sm"
            value={form.playbookBasePath}
            placeholder="playbooks"
            onChange={(event) =>
              onFormChange((prev) => ({ ...prev, playbookBasePath: event.target.value }))
            }
          />
        </FormField>
        <FormSelect
          value={form.syncPolicy}
          onValueChange={(syncPolicy) => onFormChange((prev) => ({ ...prev, syncPolicy }))}
          label={t("playbooks.page.projects.syncPolicy")}
          options={["manual", "on_launch", "scheduled"].map((policy) => ({
            value: policy,
            label: t(`playbooks.page.projects.syncPolicyValue.${policy}`),
          }))}
        />
        <FormField
          label={t("playbooks.page.fields.executionEnvironment")}
          className="md:col-span-2"
        >
          <SearchableSelect
            value={form.defaultExecutionEnvironmentId}
            onValueChange={(defaultExecutionEnvironmentId) =>
              onFormChange((prev) => ({ ...prev, defaultExecutionEnvironmentId }))
            }
            disabled={submitting || executionEnvironments.length === 0}
            items={executionEnvironments.map((environment) => ({
              id: environment.id,
              label: environment.name,
              searchText: environment.image,
            }))}
            placeholder={t(
              executionEnvironments.length === 0
                ? "playbooks.page.placeholders.noExecutionEnvironments"
                : "playbooks.page.placeholders.executionEnvironment",
            )}
            searchPlaceholder={t("search")}
          />
        </FormField>
      </fieldset>
    </EntityEditorPage>
  );
}
