import { useTranslation } from "react-i18next";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type {
  PlaybookCatalogItem,
  PlaybookExecutionEnvironment,
  PlaybookProject,
  PlaybookTargetSet,
} from "@/features/playbooks/api";

import { FormField, ToggleField } from "./PlaybookFormFields";
import { targetSetLabel } from "./playbookPageFormat";
import type { JobTemplateForm, JobTemplateSourceKind } from "./playbookFormModels";

export function JobTemplateEditor({
  form,
  playbooks,
  projects,
  targetSets,
  executionEnvironments,
  onFormChange: changeDraft,
  onBack,
  onSubmit,
  submitting,
  dirty = false,
}: Readonly<{
  form: JobTemplateForm;
  playbooks: PlaybookCatalogItem[];
  projects: PlaybookProject[];
  targetSets: PlaybookTargetSet[];
  executionEnvironments: PlaybookExecutionEnvironment[];
  onFormChange: React.Dispatch<React.SetStateAction<JobTemplateForm>>;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
}>) {
  const { t } = useTranslation();
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  return (
    <EntityEditorPage scope="routed"
      title={t("playbooks.page.dialogs.jobTemplate.title")}
      description={t("playbooks.page.dialogs.jobTemplate.description")}
      onBack={() => { if (!submitting) onBack(); }}
      backLabel={t("back")}
      onSave={onSubmit}
      saveLabel={t("create")}
      saving={submitting}
      saveDisabled={
        !form.name.trim() ||
        (form.sourceKind === "playbook" && !form.playbookId) ||
        (form.sourceKind === "project" && (!form.projectId || !form.playbookPath.trim()))
      }
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
        <FormSelect<JobTemplateSourceKind>
          value={form.sourceKind}
          onValueChange={(sourceKind) => onFormChange((prev) => ({ ...prev, sourceKind }))}
          label={t("playbooks.page.jobTemplates.source")}
          options={[
            {
              value: "playbook",
              label: t("playbooks.page.jobTemplates.sourcePlaybook"),
            },
            {
              value: "project",
              label: t("playbooks.page.jobTemplates.sourceProject"),
            },
          ]}
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
        {form.sourceKind === "playbook" ? (
          <FormField label={t("playbooks.page.fields.playbook")} className="md:col-span-2">
            <SearchableSelect
              value={form.playbookId}
              onValueChange={(playbookId) => onFormChange((prev) => ({ ...prev, playbookId }))}
              disabled={submitting || playbooks.length === 0}
              items={playbooks.map((item) => ({
                id: item.playbook.id,
                label: item.playbook.name,
                searchText: item.playbook.slug,
              }))}
              placeholder={t("playbooks.page.placeholders.playbook")}
              searchPlaceholder={t("search")}
            />
          </FormField>
        ) : (
          <>
            <FormField label={t("playbooks.page.projects.title")}>
              <SearchableSelect
                value={form.projectId}
                onValueChange={(projectId) => onFormChange((prev) => ({ ...prev, projectId }))}
                disabled={submitting || projects.length === 0}
                items={projects.map((project) => ({
                  id: project.id,
                  label: project.name,
                  searchText: project.scm_url ?? project.scm_type,
                }))}
                placeholder={t("playbooks.page.placeholders.project")}
                searchPlaceholder={t("search")}
              />
            </FormField>
            <FormField label={t("playbooks.page.jobTemplates.playbookPath")}>
              <Input
                className="h-9 font-mono text-sm"
                value={form.playbookPath}
                onChange={(event) =>
                  onFormChange((prev) => ({ ...prev, playbookPath: event.target.value }))
                }
              />
            </FormField>
          </>
        )}
        <FormField label={t("playbooks.page.fields.targetSet")}>
          <SearchableSelect
            value={form.targetSetId}
            onValueChange={(targetSetId) => onFormChange((prev) => ({ ...prev, targetSetId }))}
            disabled={submitting || targetSets.length === 0}
            items={targetSets.map((targetSet) => ({
              id: targetSet.id,
              label: targetSetLabel(targetSet),
              searchText: targetSet.selector ? JSON.stringify(targetSet.selector) : targetSet.mode,
            }))}
            placeholder={t(
              targetSets.length === 0
                ? "playbooks.page.placeholders.noTargetSets"
                : "playbooks.page.placeholders.targetSet",
            )}
            searchPlaceholder={t("search")}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.executionEnvironment")}>
          <SearchableSelect
            value={form.executionEnvironmentId}
            onValueChange={(executionEnvironmentId) =>
              onFormChange((prev) => ({ ...prev, executionEnvironmentId }))
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
