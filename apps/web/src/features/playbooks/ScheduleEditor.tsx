import { useState } from "react";
import { useTranslation } from "react-i18next";
import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { Textarea } from "@/shared/ui-primitives/textarea";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import type { JobTemplate, JobTemplateVarsSchema, PlaybookTargetSetListItem } from "./api";
import { FormField, ToggleField } from "./PlaybookFormFields";
import { concurrencyPolicies, type ScheduleForm } from "./playbookFormModels";
import { TargetSetPicker, type TargetSetPreviewState } from "./TargetSetPicker";
import { fixedSnapshotTargets, scheduleDraftErrorDefaults, scheduleTargetsError, scheduleTargetsErrorDefaults, validateScheduleDraft } from "./scheduleModel";
import TargetSetSelectorBuilder from "./TargetSetSelectorBuilder";
import { emptySelectorModel, modelToSelector, selectorToModel } from "./selectorModel";
import ParametersEditor from "./ParametersEditor";
import { jobVarsSchema, parseParameterValues, type Extravars } from "./varsSchema";
import type { LaunchResource } from "./launchReadiness";

export function ScheduleEditor({ form, jobTemplates, targetSets, varsSchema, onFormChange: changeDraft, onBack, submitting, dirty = false, referenceError, editing = false, onSave, onSelectTargetSet, onRefreshSnapshot, snapshotPending = false, targetSetPreview, onCreateTargetSet, onBrowseTargetSets }: Readonly<{
  form: ScheduleForm;
  jobTemplates: JobTemplate[];
  varsSchema?: LaunchResource<JobTemplateVarsSchema>;
  targetSets: PlaybookTargetSetListItem[];
  onFormChange: React.Dispatch<React.SetStateAction<ScheduleForm>>;
  onBack: () => void;
  submitting: boolean;
  dirty?: boolean;
  referenceError?: string;
  editing?: boolean;
  onSave?: () => void;
  onSelectTargetSet?: (id: string) => void;
  /** Opens the snapshot diff for a fixed schedule copied from a target set. */
  onRefreshSnapshot?: () => void;
  snapshotPending?: boolean;
  targetSetPreview?: TargetSetPreviewState;
  /** Opens the target-set create flow; the owner passes it only with the create permission. */
  onCreateTargetSet?: () => void;
  /** Opens the target-set list; the owner passes it only with the read permission. */
  onBrowseTargetSets?: () => void;
}>) {
  const { t } = useTranslation();
  const [targetsTouched, setTargetsTouched] = useState(false);
  const [parametersValid, setParametersValid] = useState(true);
  const selectedJob = jobTemplates.find((job) => job.id === form.jobTemplateId);
  const schema = jobVarsSchema(selectedJob?.vars_schema, varsSchema?.status === "ready" ? varsSchema.data : undefined);
  const schemaReady = selectedJob?.vars_schema?.kind !== "project_file" || varsSchema?.status === "ready";
  let jobValues: Extravars = {};
  let savedParametersValid = true;
  try { jobValues = parseParameterValues(JSON.stringify(selectedJob?.options.extra_vars ?? {})); } catch { savedParametersValid = false; }
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  const targets = targetSets.filter((target) => target.mode === form.selectionMode);
  const errors = dirty ? validateScheduleDraft(form) : [];
  const targetsError = targetsTouched ? scheduleTargetsError(form) : null;
  const snapshotTargets = fixedSnapshotTargets(form);
  let selectorModel;
  try {
    const selector: unknown = JSON.parse(form.targetsJson);
    if (selector && typeof selector === "object" && !Array.isArray(selector)) selectorModel = selectorToModel(selector);
  } catch { /* An invalid draft remains editable and is rejected on save. */ }
  return <EntityEditorPage scope="routed" title={t(editing ? "playbooks.page.dialogs.schedule.editTitle" : "playbooks.page.dialogs.schedule.title")} description={t("playbooks.scheduleDraft.description", { defaultValue: "Schedule a saved job template with a target selection." })}
    onBack={() => { if (!submitting) onBack(); }} backLabel={t("back")}
    saving={submitting} onSave={onSave} saveDisabled={!parametersValid || !savedParametersValid || !schemaReady} saveLabel={t("save")} onCancel={onBack} cancelLabel={t("cancel")} dirty={dirty} dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}>
    <fieldset disabled={submitting} inert={submitting} className="grid min-w-0 gap-4 md:grid-cols-2">
      {referenceError && <p role="alert" className="text-sm text-status-warning md:col-span-2">{referenceError}</p>}
      {!savedParametersValid && <p role="alert" className="text-sm text-destructive md:col-span-2">{t("playbooks.parameters.savedInvalid", { defaultValue: "The saved parameters are invalid. Edit the job before launching." })}</p>}
      <p className="text-sm text-muted-foreground md:col-span-2">{t("playbooks.scheduleDraft.ownerRequired", { defaultValue: "The schedule runs with its owner's current permissions." })}</p>
      <FormField label={t("name")}><Input value={form.name} onChange={(event) => onFormChange((prev) => ({ ...prev, name: event.target.value }))} /></FormField>
      {selectedJob && schemaReady && <div className="md:col-span-2"><ParametersEditor key={selectedJob.id} schema={schema} inherited={jobValues} value={form.extraVars} onChange={(extraVars) => onFormChange((prev) => ({ ...prev, extraVars }))} onValidityChange={setParametersValid} disabled={submitting} /></div>}
      {!schemaReady && <p role="status" className="text-sm text-status-warning md:col-span-2">{t(varsSchema?.status === "failed" ? "playbooks.parameters.schemaFailed" : "playbooks.parameters.schemaLoading")}</p>}
      <FormField label={t("playbooks.scheduleDraft.job", { defaultValue: "Job template" })}>{editing ? <Input readOnly value={form.jobTemplateId} /> : <SearchableSelect value={form.jobTemplateId} onValueChange={(jobTemplateId) => onFormChange((prev) => ({ ...prev, jobTemplateId }))} items={jobTemplates.map((job) => ({ id: job.id, label: job.name }))} placeholder={t("playbooks.scheduleDraft.chooseJob", { defaultValue: "Select a job template" })} searchPlaceholder={t("search")} />}</FormField>
      <FormSelect value={form.selectionMode} onValueChange={(selectionMode: "static" | "dynamic") => onFormChange((prev) => ({ ...prev, selectionMode, targetSetId: "", targetsJson: selectionMode === "dynamic" ? JSON.stringify(modelToSelector(emptySelectorModel()), null, 2) : "" }))} label={t("playbooks.scheduleDraft.targets", { defaultValue: "Target selection" })} options={[
        { value: "static", label: t("playbooks.scheduleDraft.fixedSnapshot", { defaultValue: "Fixed snapshot" }) },
        { value: "dynamic", label: t("playbooks.scheduleDraft.dynamicSelector", { defaultValue: "Dynamic selector" }) },
      ]} />
      <p className="text-sm text-muted-foreground md:col-span-2">{t(form.selectionMode === "static" ? "playbooks.scheduleDraft.snapshotHint" : "playbooks.scheduleDraft.dynamicHint", { defaultValue: form.selectionMode === "static" ? "The host list is frozen when you save. Editing the target set does not change this schedule; update the snapshot from the set to apply a new composition." : "The composition is recomputed from the saved selector on every firing. Editing the target set does not rewrite the saved selector; select the set again to copy its current rules." })}</p>
      <FormField label={form.selectionMode === "static" ? t("playbooks.scheduleDraft.fixedSnapshotSet", { defaultValue: "Snapshot target set" }) : t("playbooks.scheduleDraft.dynamicSet", { defaultValue: "Dynamic target set" })}><TargetSetPicker targetSets={targets} value={form.targetSetId} onValueChange={(targetSetId) => onSelectTargetSet?.(targetSetId)} preview={targetSetPreview} disabled={submitting} placeholder={t("playbooks.page.placeholders.targetSet")} onCreate={onCreateTargetSet} onBrowse={onBrowseTargetSets} /></FormField>
      {form.selectionMode === "static" && <div className="flex min-w-0 flex-wrap items-center gap-2">
        {snapshotTargets && <p className="text-sm text-muted-foreground">{t("playbooks.scheduleDraft.snapshotCount", { count: snapshotTargets.length, defaultValue: "Hosts in snapshot: {{count}}" })}</p>}
        {onRefreshSnapshot && form.targetSetId && <Button type="button" variant="outline" size="sm" disabled={snapshotPending} onClick={onRefreshSnapshot}>{t("playbooks.scheduleDraft.updateSnapshot", { defaultValue: "Update snapshot from set" })}</Button>}
      </div>}
      {form.selectionMode === "dynamic" && selectorModel && <div className="md:col-span-2"><TargetSetSelectorBuilder model={selectorModel} onChange={(next) => onFormChange((prev) => ({ ...prev, targetsJson: JSON.stringify(modelToSelector(next), null, 2), targetSetId: "" }))} /></div>}
      <details open={targetsError ? true : undefined} className="min-w-0 rounded-md border border-border bg-card p-3 md:col-span-2">
        <summary className="cursor-pointer text-sm font-medium text-foreground">{t("playbooks.scheduleDraft.advanced", { defaultValue: "Advanced: targets as JSON" })}</summary>
        <p className="mt-2 text-xs text-muted-foreground">{t("playbooks.scheduleDraft.advancedHint", { defaultValue: "For non-standard cases. The normal path is the target set and structured fields above." })}</p>
        <FormField className="mt-2" label={form.selectionMode === "static" ? t("playbooks.scheduleDraft.fixedJson", { defaultValue: "Resolved fixed targets (JSON)" }) : t("playbooks.scheduleDraft.selectorJson", { defaultValue: "Dynamic selector (JSON)" })}>
          <Textarea aria-invalid={Boolean(targetsError)} value={form.targetsJson} onChange={(event) => { setTargetsTouched(true); onFormChange((prev) => ({ ...prev, targetsJson: event.target.value, targetSetId: "" })); }} />
          {targetsError && <p role="alert" className="text-sm text-status-warning">{t(`playbooks.scheduleDraft.targetsErrors.${targetsError}`, { defaultValue: scheduleTargetsErrorDefaults[targetsError] })}</p>}
        </FormField>
      </details>
      <FormField label={t("playbooks.page.schedules.cron")}><Input value={form.cronExpression} onChange={(event) => onFormChange((prev) => ({ ...prev, cronExpression: event.target.value }))} placeholder="0 3 * * *" /></FormField>
      <FormField label={t("playbooks.page.schedules.timezone")}><Input value={form.timezone} onChange={(event) => onFormChange((prev) => ({ ...prev, timezone: event.target.value }))} placeholder="UTC" /></FormField>
      <FormSelect value={form.concurrencyPolicy} onValueChange={(concurrencyPolicy) => onFormChange((prev) => ({ ...prev, concurrencyPolicy }))} label={t("playbooks.page.schedules.concurrency")} options={concurrencyPolicies.map((policy) => ({ value: policy, label: t(`playbooks.page.schedules.concurrencyPolicy.${policy}`) }))} />
      <FormField label={t("playbooks.page.schedules.misfire")}><Input value={t(`playbooks.page.schedules.misfirePolicy.${form.misfirePolicy}`, { defaultValue: form.misfirePolicy })} readOnly /></FormField>
      <ToggleField label={t("enabled")} checked={form.enabled} onCheckedChange={(enabled) => onFormChange((prev) => ({ ...prev, enabled }))} />
      {errors.length > 0 && <ul className="list-disc pl-4 text-sm text-status-warning md:col-span-2">{errors.map((error) => <li key={error}>{t(`playbooks.scheduleDraft.errors.${error}`, { defaultValue: scheduleDraftErrorDefaults[error] })}</li>)}</ul>}
    </fieldset>
  </EntityEditorPage>;
}
