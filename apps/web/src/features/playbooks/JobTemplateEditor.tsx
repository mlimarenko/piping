import { useState } from "react";
import { useTranslation } from "react-i18next";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type {
  PlaybookExecutionEnvironment,
  PlaybookTargetSetListItem,
  Project,
  ProjectEntry,
} from "@/features/playbooks/api";
import type { AutomationSecret } from "@/features/automation-secrets/api";

import { FormField } from "./PlaybookFormFields";
import { TargetSetPicker, type TargetSetPreviewState } from "./TargetSetPicker";
import type { JobTemplateForm } from "./playbookFormModels";
import { jobCommandPreview, jobDraftFieldErrors } from "./jobTemplateModel";
import { EntrypointsField, EnvironmentField, JobFieldError, SecretReferencesField } from "./JobDraftFields";
import { AnsibleOptionsFields } from "./AnsibleOptionsFields";
import { GitSourceSummary } from "./GitSourceSummary";
import ParametersEditor from "./ParametersEditor";
import { NO_PARAMETERS_SCHEMA, projectFileVarsSchema } from "./varsSchema";

export function JobTemplateEditor({
  form,
  projects,
  targetSets,
  executionEnvironments,
  entrypointFiles = [],
  projectEntries = [],
  entrypointFilesLoading = false,
  entrypointFilesError,
  projectRevisionMissing = false,
  secrets = [],
  secretsLoading = false,
  secretsError,
  onFormChange: changeDraft,
  onBack,
  submitting,
  referenceError,
  dirty = false,
  onSave,
  editing = false,
  targetSetPreview,
  onCreateTargetSet,
  onBrowseTargetSets,
}: Readonly<{
  form: JobTemplateForm;
  projects: Project[];
  targetSets: PlaybookTargetSetListItem[];
  executionEnvironments: PlaybookExecutionEnvironment[];
  entrypointFiles?: string[];
  projectEntries?: ProjectEntry[];
  entrypointFilesLoading?: boolean;
  entrypointFilesError?: string;
  projectRevisionMissing?: boolean;
  secrets?: AutomationSecret[];
  secretsLoading?: boolean;
  secretsError?: string;
  onFormChange: React.Dispatch<React.SetStateAction<JobTemplateForm>>;
  onBack: () => void;
  submitting: boolean;
  referenceError?: string;
  dirty?: boolean;
  onSave?: () => void;
  editing?: boolean;
  targetSetPreview?: TargetSetPreviewState;
  /** Opens the target-set create flow; the owner passes it only with the create permission. */
  onCreateTargetSet?: () => void;
  /** Opens the target-set list; the owner passes it only with the read permission. */
  onBrowseTargetSets?: () => void;
}>) {
  const { t } = useTranslation();
  const [parametersValid, setParametersValid] = useState(true);
  let schema: unknown = NO_PARAMETERS_SCHEMA;
  let schemaValid = true;
  try {
    if (form.varsSchemaKind === "inline") schema = JSON.parse(form.varsSchema);
    if (form.varsSchemaKind === "project_file") {
      schema = projectFileVarsSchema(projectEntries, form.varsSchemaPath);
    }
  } catch { schemaValid = false; }
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  const errors = jobDraftFieldErrors(form);
  const project = projects.find((item) => item.id === form.projectId);
  const gitSource = project?.source.kind === "git";
  const fieldError = (field: keyof typeof errors) => (dirty ? errors[field] : undefined);
  const set = <K extends keyof JobTemplateForm>(key: K, value: JobTemplateForm[K]) => onFormChange((prev) => ({ ...prev, [key]: value }));

  return (
    <EntityEditorPage scope="routed"
      title={t(editing ? "playbooks.page.dialogs.jobTemplate.editTitle" : "playbooks.page.dialogs.jobTemplate.title")}
      description={t("playbooks.page.dialogs.jobTemplate.description")}
      onBack={() => { if (!submitting) onBack(); }}
      backLabel={t("back")}
      saving={submitting}
      onSave={onSave}
      saveDisabled={!parametersValid || !schemaValid}
      saveLabel={t("save")}
      onCancel={onBack}
      cancelLabel={t("cancel")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
    >
      <fieldset disabled={submitting} inert={submitting} className="grid min-w-0 gap-4 md:grid-cols-2">
        {referenceError && <p role="alert" className="text-sm text-status-warning md:col-span-2">{referenceError}</p>}
        <FormField label={t("name")}>
          <Input
            className="h-9 text-sm"
            aria-invalid={fieldError("name") ? true : undefined}
            value={form.name}
            onChange={(event) => set("name", event.target.value)}
          />
          <JobFieldError code={fieldError("name")} />
        </FormField>
        <FormField label={t("playbooks.page.projects.title")}>
          <SearchableSelect
            value={form.projectId}
            onValueChange={(projectId) => onFormChange((prev) => prev.projectId === projectId ? prev : { ...prev, projectId, playbookPath: "[]", revisionPolicy: "current", revision: "" })}
            disabled={submitting || projects.length === 0}
            items={projects.map((project) => ({
              id: project.id,
              label: project.name,
            }))}
            placeholder={t("playbooks.page.placeholders.project")}
            searchPlaceholder={t("search")}
          />
          <JobFieldError code={fieldError("project")} />
        </FormField>
        <EntrypointsField
          value={form.playbookPath}
          onChange={(playbookPath) => set("playbookPath", playbookPath)}
          files={entrypointFiles}
          filesLoading={entrypointFilesLoading}
          filesError={entrypointFilesError}
          projectSelected={Boolean(form.projectId)}
          projectRevisionMissing={projectRevisionMissing}
          error={fieldError("entrypoints")}
          disabled={submitting}
        />
        <FormSelect value={form.revisionPolicy} onValueChange={(revisionPolicy) => onFormChange((prev) => ({ ...prev, revisionPolicy, revision: revisionPolicy === "fixed" ? prev.revision || project?.current_revision_id || "" : prev.revision }))} label={t("playbooks.jobDraft.revisionPolicy", { defaultValue: "Revision policy" })} options={[
          { value: "current", label: t(gitSource ? "playbooks.gitSource.follow" : "playbooks.jobDraft.projectDefault") },
          { value: "fixed", label: t(gitSource ? "playbooks.gitSource.pin" : "playbooks.jobDraft.pinned") },
        ]} />
        {form.revisionPolicy === "fixed" && <FormField label={t("playbooks.jobDraft.revision", { defaultValue: "Revision" })}><Input value={form.revision} onChange={(event) => set("revision", event.target.value)} /><JobFieldError code={fieldError("revision")} /></FormField>}
        {gitSource && project && <div className="min-w-0 md:col-span-2"><GitSourceSummary project={project} selection={form.revisionPolicy === "fixed" ? { kind: "fixed", revision_id: form.revision } : { kind: "current" }} disabled={submitting} /></div>}
        <FormField label={t("playbooks.jobDraft.workdir", { defaultValue: "Working directory" })}><Input value={form.workingDirectory} onChange={(event) => set("workingDirectory", event.target.value)} /><JobFieldError code={fieldError("workdir")} /></FormField>
        <FormSelect value={form.inventoryMode} onValueChange={(inventoryMode: "managed" | "native") => onFormChange((prev) => ({ ...prev, inventoryMode }))} label={t("playbooks.jobDraft.inventoryMode", { defaultValue: "Inventory source" })} options={[
          { value: "managed", label: t("playbooks.jobDraft.managedInventory", { defaultValue: "Managed inventory" }) },
          { value: "native", label: t("playbooks.jobDraft.nativeInventory", { defaultValue: "Project inventory" }) },
        ]} />
        {form.inventoryMode === "native" && <><FormField label={t("playbooks.jobDraft.inventorySourcesJson", { defaultValue: "Inventory sources (JSON array)" })}><Textarea value={form.inventory} onChange={(event) => set("inventory", event.target.value)} /><JobFieldError code={fieldError("inventory")} /></FormField><FormField label={t("playbooks.jobDraft.sshRecord", { defaultValue: "SSH record ID (optional)" })}><Input value={form.sshRecordId} onChange={(event) => set("sshRecordId", event.target.value)} /><JobFieldError code={fieldError("sshRecord")} /></FormField></>}
        <AnsibleOptionsFields value={form.argumentsText} onChange={(argumentsText) => set("argumentsText", argumentsText)} limit={form.limit} onLimitChange={(limit) => set("limit", limit)} disabled={submitting} />
        <EnvironmentField value={form.environment} onChange={(environment) => set("environment", environment)} error={fieldError("environment")} disabled={submitting} />
        <FormSelect label={t("playbooks.parameters.source", { defaultValue: "Parameter declarations" })} value={form.varsSchemaKind} onValueChange={(kind) => set("varsSchemaKind", kind)} options={[
          { value: "none", label: t("playbooks.parameters.sourceNone", { defaultValue: "No parameters" }) },
          { value: "inline", label: t("playbooks.parameters.sourceInline", { defaultValue: "Declare in this job" }) },
          { value: "project_file", label: t("playbooks.parameters.sourceFile", { defaultValue: "Use a project schema file" }) },
        ]} />
        {form.varsSchemaKind === "project_file" && <FormField label={t("playbooks.parameters.schemaFile", { defaultValue: "JSON Schema file" })}><SearchableSelect value={form.varsSchemaPath} onValueChange={(path) => set("varsSchemaPath", path)} items={entrypointFiles.map((path) => ({ id: path, label: path }))} disabled={entrypointFilesLoading} searchPlaceholder={t("search")} /></FormField>}
        {form.varsSchemaKind === "inline" && <FormField label={t("playbooks.parameters.schema", { defaultValue: "Parameter declarations (JSON Schema)" })} className="md:col-span-2"><Textarea className="min-h-32 font-mono text-sm" value={form.varsSchema} onChange={(event) => set("varsSchema", event.target.value)} /><JobFieldError code={fieldError("varsSchema")} /></FormField>}
        {!schemaValid && <p role="alert" className="text-sm text-destructive md:col-span-2">{t("playbooks.parameters.schemaInvalid", { defaultValue: "Select a valid JSON Schema from this revision, or fix the inline declaration." })}</p>}
        {schemaValid && <div className="md:col-span-2"><ParametersEditor key={form.varsSchemaKind + form.varsSchemaPath} schema={schema} value={form.extraVars} onChange={(value) => set("extraVars", value)} onValidityChange={setParametersValid} requireAll={false} disabled={submitting} /></div>}
        <SecretReferencesField
          value={form.secretReferences}
          onChange={(secretReferences) => set("secretReferences", secretReferences)}
          secrets={secrets}
          secretsLoading={secretsLoading}
          secretsError={secretsError}
          error={fieldError("secretReferences")}
          disabled={submitting}
        />
        <div className="md:col-span-2"><p className="text-sm font-medium">{t("playbooks.jobDraft.preview", { defaultValue: "Command preview (arguments and variables hidden)" })}</p><pre className="overflow-auto whitespace-pre-wrap text-sm">{jobCommandPreview(form)}</pre></div>
        {form.inventoryMode === "managed" && <FormField label={t("playbooks.page.fields.targetSet")}>
          <TargetSetPicker
            targetSets={targetSets}
            value={form.targetSetId}
            onValueChange={(targetSetId) => onFormChange((prev) => ({ ...prev, targetSetId }))}
            preview={targetSetPreview}
            disabled={submitting}
            placeholder={t(
              targetSets.length === 0
                ? "playbooks.page.placeholders.noTargetSets"
                : "playbooks.page.placeholders.targetSet",
            )}
            onCreate={onCreateTargetSet}
            onBrowse={onBrowseTargetSets}
          />
          <JobFieldError code={fieldError("targetSet")} />
        </FormField>}
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
          <JobFieldError code={fieldError("executionEnvironment")} />
        </FormField>
        <details className="md:col-span-2"><summary className="cursor-pointer text-sm font-medium">{t("playbooks.jobDraft.advanced", { defaultValue: "Advanced options" })}</summary><div className="grid gap-4 pt-4 md:grid-cols-2">
          <FormField label={t("playbooks.jobDraft.timeout", { defaultValue: "Timeout (seconds)" })}><Input inputMode="numeric" value={form.timeoutSeconds} onChange={(event) => set("timeoutSeconds", event.target.value)} /><JobFieldError code={fieldError("timeout")} /></FormField>
          <FormField label={t("playbooks.jobDraft.entrypointsJson", { defaultValue: "Entrypoints (JSON array of project-relative paths)" })} className="md:col-span-2"><Textarea className="font-mono text-sm" value={form.playbookPath} onChange={(event) => set("playbookPath", event.target.value)} /></FormField>
          <FormField label={t("playbooks.jobDraft.envJson", { defaultValue: "Environment (JSON object)" })} className="md:col-span-2"><Textarea className="font-mono text-sm" value={form.environment} onChange={(event) => set("environment", event.target.value)} /></FormField>
          <FormField label={t("playbooks.jobDraft.secretRefsJson", { defaultValue: "Secret references (JSON object; no secret values)" })} className="md:col-span-2"><Textarea className="font-mono text-sm" value={form.secretReferences} onChange={(event) => set("secretReferences", event.target.value)} /></FormField>
        </div></details>
      </fieldset>
    </EntityEditorPage>
  );
}
