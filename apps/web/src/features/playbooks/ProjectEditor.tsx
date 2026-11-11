import { useTranslation } from "react-i18next";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import { FormField } from "./PlaybookFormFields";
import { ProjectAuthFields } from "./ProjectAuthFields";
import type { ProjectAuthOptions } from "./useProjectAuthOptions";
import { validProjectForm, type ProjectForm } from "./projectFormModel";

export function ProjectEditor({ form, onFormChange: changeDraft, onBack, onSubmit, submitting, dirty = false, canCreate = true, authOptions }: Readonly<{
  form: ProjectForm;
  onFormChange: React.Dispatch<React.SetStateAction<ProjectForm>>;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
  canCreate?: boolean;
  authOptions: ProjectAuthOptions;
}>) {
  const { t } = useTranslation();
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  const back = () => { if (!submitting) onBack(); };
  return <EntityEditorPage scope="routed" title={t("playbooks.page.dialogs.project.title")} description={t("playbooks.page.dialogs.project.description")} onBack={back} backLabel={t("back")} onSave={onSubmit} saveLabel={t("create")} saving={submitting} saveDisabled={!canCreate || !validProjectForm(form)} onCancel={back} cancelLabel={t("cancel")} dirty={dirty} dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}>
    <fieldset disabled={submitting} inert={submitting} className="grid min-w-0 gap-4 md:grid-cols-2">
      <FormField label={t("name")}><Input value={form.name} onChange={(e) => onFormChange((p) => ({ ...p, name: e.target.value }))} /></FormField>
      <FormSelect value={form.sourceKind} onValueChange={(value) => onFormChange((p) => ({ ...p, sourceKind: value }))} label={t("playbooks.page.projects.scmTypeLabel")} options={[{ value: "editable", label: t("playbooks.page.projects.scmType.manual") }, { value: "git", label: "Git" }]} />
      <FormField label={t("playbooks.page.fields.description")} className="md:col-span-2"><Input value={form.description} onChange={(e) => onFormChange((p) => ({ ...p, description: e.target.value }))} /></FormField>
      {form.sourceKind === "git" && <>
        <FormField label={t("playbooks.page.projects.scmUrl")} className="md:col-span-2"><Input value={form.url} onChange={(e) => onFormChange((p) => ({ ...p, url: e.target.value }))} /></FormField>
        <FormField label={t("playbooks.page.projects.ref")}><Input value={form.reference} onChange={(e) => onFormChange((p) => ({ ...p, reference: e.target.value }))} /></FormField>
        <ProjectAuthFields form={form} onChange={onFormChange} options={authOptions} disabled={submitting} />
      </>}
    </fieldset>
  </EntityEditorPage>;
}
