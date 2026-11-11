import { useTranslation } from "react-i18next";
import { ChevronDown } from "lucide-react";

import { Button } from "@/shared/ui-primitives/button";
import EntityEditorHost from "@/shared/ui-shared/EntityEditorHost";
import { Input } from "@/shared/ui-primitives/input";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/shared/ui-primitives/collapsible";
import { TooltipProvider } from "@/shared/ui-primitives/tooltip";
import type {
  JobTemplate,
  PlaybookTargetSetListItem,
} from "@/features/playbooks/api";
import { FieldHelp } from "./FieldHelp";
import { automationDocUrl } from "@/shared/lib/automationDocs";
import PlaybookScopeFields from "./PlaybookScopeFields";
import { FormField, SectionLabel, ToggleField } from "./PlaybookFormFields";
import { TargetSetPicker, type TargetSetPreviewState } from "./TargetSetPicker";
import type { AssignmentForm } from "./assignmentFormModel";
import type { PlaybookScopeEntities } from "./usePlaybookScopeEntities";

export function AssignmentDialog({
  open,
  editing,
  form,
  onFormChange: changeDraft,
  playbooks,
  allTargetSets,
  targetSetMissing,
  targetSetPreview,
  onCreateTargetSet,
  scopeEntities,
  onOpenChange,
  onSubmit,
  submitting,
  error,
}: Readonly<{
  open: boolean;
  editing: boolean;
  form: AssignmentForm;
  onFormChange: React.Dispatch<React.SetStateAction<AssignmentForm>>;
  playbooks: JobTemplate[];
  allTargetSets: PlaybookTargetSetListItem[];
  targetSetMissing: boolean;
  targetSetPreview: TargetSetPreviewState;
  /** Opens the target-set create flow; the owner passes it only with the create permission. */
  onCreateTargetSet?: () => void;
  scopeEntities: PlaybookScopeEntities;
  onOpenChange: (open: boolean) => void;
  onSubmit: () => void;
  submitting: boolean;
  error?: string;
}>) {
  const { t } = useTranslation();
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  return (
    <EntityEditorHost open={open} onOpenChange={(next) => { if (!submitting) onOpenChange(next); }}
      title={t(editing ? "playbooks.page.dialogs.assignment.editTitle" : "playbooks.page.dialogs.assignment.title")}
      description={t("playbooks.page.dialogs.assignment.description")}
      footer={<>
        <Button data-editor-cancel variant="outline" disabled={submitting} onClick={() => onOpenChange(false)}>{t("cancel")}</Button>
        <Button onClick={onSubmit} disabled={submitting || !form.jobTemplateId || !form.targetSetId}>
          {editing ? t("save") : t("create")}
        </Button>
      </>}>
        <TooltipProvider delayDuration={200}>
          <fieldset disabled={submitting} inert={submitting} className="flex min-w-0 flex-col gap-4 py-2">
            {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
            {/* Basics */}
            <section className="space-y-2">
              <SectionLabel>{t("playbooks.page.dialogs.assignment.sectionBasics")}</SectionLabel>
              <div className="grid gap-3 md:grid-cols-2">
                <FormField label={t("playbooks.scheduleDraft.job", { defaultValue: "Job template" })}>
                  <SearchableSelect
                    disabled={submitting}
                    value={form.jobTemplateId}
                    onValueChange={(value) => onFormChange((prev) => ({ ...prev, jobTemplateId: value }))}
                    items={playbooks.map((item) => ({
                      id: item.id,
                      label: item.name,
                    }))}
                    placeholder={t("playbooks.scheduleDraft.chooseJob", { defaultValue: "Select a job template" })}
                    searchPlaceholder={t("search")}
                  />
                </FormField>
                <FormField label={t("name")}>
                  <Input value={form.name} onChange={(event) => onFormChange((prev) => ({ ...prev, name: event.target.value }))} />
                </FormField>
              </div>
            </section>

            {/* Targeting */}
            <section className="space-y-2">
              <SectionLabel>{t("playbooks.page.dialogs.assignment.sectionTargeting")}</SectionLabel>
              <FormField
                label={
                  <span className="flex items-center gap-1">
                    {t("playbooks.page.fields.targetSet")}
                    <FieldHelp text={t("playbooks.page.help.targetSet")} href={automationDocUrl("targets", "assignments")} />
                  </span>
                }
              >
                <TargetSetPicker
                  targetSets={allTargetSets}
                  value={form.targetSetId}
                  onValueChange={(targetSetId) => onFormChange((prev) => ({ ...prev, targetSetId }))}
                  preview={targetSetPreview}
                  disabled={submitting || targetSetMissing}
                  placeholder={t(targetSetMissing ? "playbooks.page.placeholders.noTargetSets" : "playbooks.page.placeholders.targetSet")}
                  onCreate={onCreateTargetSet}
                />
              </FormField>
              <PlaybookScopeFields
                scopeKind={form.scopeKind}
                scopeId={form.scopeId}
                onScopeKindChange={(scopeKind) => onFormChange((prev) => ({ ...prev, scopeKind }))}
                onScopeIdChange={(scopeId) => onFormChange((prev) => ({ ...prev, scopeId }))}
                entities={scopeEntities}
              />
            </section>

            {/* Behavior (advanced) */}
            <Collapsible>
              <CollapsibleTrigger className="group flex w-full items-center gap-1.5 text-left">
                <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=closed]:-rotate-90" />
                <SectionLabel>{t("playbooks.page.dialogs.assignment.sectionBehavior")}</SectionLabel>
              </CollapsibleTrigger>
              <CollapsibleContent className="space-y-3 pt-2">
                <FormField
                  label={
                    <span className="flex items-center gap-1">
                      {t("playbooks.page.fields.priority")}
                      <FieldHelp text={t("playbooks.page.help.priority")} href={automationDocUrl("targets", "assignments")} />
                    </span>
                  }
                >
                  <Input inputMode="numeric" value={form.priority} onChange={(event) => onFormChange((prev) => ({ ...prev, priority: event.target.value }))} />
                </FormField>
                <div className="grid gap-3 md:grid-cols-2">
                  <ToggleField label={t("enabled")} description={t("playbooks.page.help.enabled")} checked={form.enabled} onCheckedChange={(checked) => onFormChange((prev) => ({ ...prev, enabled: checked }))} />
                  <ToggleField label={t("playbooks.page.fields.visible")} description={t("playbooks.page.help.visible")} checked={form.visible} onCheckedChange={(checked) => onFormChange((prev) => ({ ...prev, visible: checked }))} />
                  <ToggleField label={t("playbooks.page.fields.executable")} description={t("playbooks.page.help.executable")} checked={form.executable} onCheckedChange={(checked) => onFormChange((prev) => ({ ...prev, executable: checked }))} />
                  <ToggleField label={t("playbooks.page.fields.quickAction")} description={t("playbooks.page.help.quickAction")} checked={form.quickActionEnabled} onCheckedChange={(checked) => onFormChange((prev) => ({ ...prev, quickActionEnabled: checked }))} />
                </div>
              </CollapsibleContent>
            </Collapsible>
          </fieldset>
        </TooltipProvider>
    </EntityEditorHost>
  );
}
