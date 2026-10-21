import { useTranslation } from "react-i18next";
import { ChevronDown, Plus } from "lucide-react";

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
  PlaybookCatalogItem,
  PlaybookTargetSet,
} from "@/features/playbooks/api";
import { FieldHelp } from "./FieldHelp";
import PlaybookScopeFields from "./PlaybookScopeFields";
import { FormField, SectionLabel, ToggleField } from "./PlaybookFormFields";
import { targetSetLabel } from "./playbookPageFormat";
import type { AssignmentForm } from "./playbookFormModels";
import type { PlaybookScopeEntities } from "./usePlaybookScopeEntities";

export function AssignmentDialog({
  open,
  editing,
  form,
  onFormChange: changeDraft,
  playbooks,
  allTargetSets,
  targetSetMissing,
  previewPending,
  previewError,
  previewTotal,
  canCreateTargetSet,
  canReadTargetSet,
  onCreateTargetSet,
  scopeEntities,
  onOpenChange,
  onSubmit,
  submitting,
}: Readonly<{
  open: boolean;
  editing: boolean;
  form: AssignmentForm;
  onFormChange: React.Dispatch<React.SetStateAction<AssignmentForm>>;
  playbooks: PlaybookCatalogItem[];
  allTargetSets: PlaybookTargetSet[];
  targetSetMissing: boolean;
  previewPending: boolean;
  previewError: boolean;
  previewTotal: number;
  canCreateTargetSet: boolean;
  canReadTargetSet: boolean;
  onCreateTargetSet: () => void;
  scopeEntities: PlaybookScopeEntities;
  onOpenChange: (open: boolean) => void;
  onSubmit: () => void;
  submitting: boolean;
}>) {
  const { t } = useTranslation();
  const onFormChange: typeof changeDraft = (next) => { if (!submitting) changeDraft(next); };
  let previewContent: React.ReactNode;
  if (!canReadTargetSet) {
    previewContent = t("accessDenied.title");
  } else if (previewPending) {
    previewContent = t("playbooks.launchDialog.loadingPreview");
  } else if (previewError) {
    previewContent = (
      <span className="text-status-warning">{t("playbooks.launchDialog.previewFailed")}</span>
    );
  } else {
    previewContent = t("playbooks.page.dialogs.assignment.resolvedHosts", {
      count: previewTotal,
    });
  }
  return (
    <EntityEditorHost open={open} onOpenChange={(next) => { if (!submitting) onOpenChange(next); }}
      title={t(editing ? "playbooks.page.dialogs.assignment.editTitle" : "playbooks.page.dialogs.assignment.title")}
      description={t("playbooks.page.dialogs.assignment.description")}
      footer={<>
        <Button data-editor-cancel variant="outline" disabled={submitting} onClick={() => onOpenChange(false)}>{t("cancel")}</Button>
        <Button onClick={onSubmit} disabled={submitting || !form.playbookId || !form.targetSetId}>
          {editing ? t("save") : t("create")}
        </Button>
      </>}>
        <TooltipProvider delayDuration={200}>
          <fieldset disabled={submitting} inert={submitting} className="flex min-w-0 flex-col gap-4 py-2">
            {/* Basics */}
            <section className="space-y-2">
              <SectionLabel>{t("playbooks.page.dialogs.assignment.sectionBasics")}</SectionLabel>
              <div className="grid gap-3 md:grid-cols-2">
                <FormField label={t("playbooks.page.fields.playbook")}>
                  <SearchableSelect
                    disabled={submitting}
                    value={form.playbookId}
                    onValueChange={(value) => onFormChange((prev) => ({ ...prev, playbookId: value }))}
                    items={playbooks.map((item) => ({
                      id: item.playbook.id,
                      label: item.playbook.name,
                      searchText: item.playbook.slug,
                    }))}
                    placeholder={t("playbooks.page.placeholders.playbook")}
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
                    <FieldHelp text={t("playbooks.page.help.targetSet")} />
                  </span>
                }
              >
                <div className="space-y-1.5">
                  <SearchableSelect
                    value={form.targetSetId}
                    onValueChange={(value) => onFormChange((prev) => ({ ...prev, targetSetId: value }))}
                    disabled={submitting || targetSetMissing}
                    items={allTargetSets.map((targetSet) => ({
                      id: targetSet.id,
                      label: targetSetLabel(targetSet),
                      searchText: targetSet.selector ? JSON.stringify(targetSet.selector) : targetSet.mode,
                    }))}
                    placeholder={t(targetSetMissing ? "playbooks.page.placeholders.noTargetSets" : "playbooks.page.placeholders.targetSet")}
                    searchPlaceholder={t("search")}
                  />
                  {form.targetSetId ? (
                    <div className="rounded-md border border-border bg-background px-3 py-2 text-xs text-muted-foreground">
                      {previewContent}
                    </div>
                  ) : null}
                  {canCreateTargetSet && (
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      className="gap-2 px-2 text-primary hover:text-primary"
                      onClick={onCreateTargetSet}
                      disabled={submitting}
                    >
                      <Plus className="h-3 w-3" />
                      {t("playbooks.page.actions.newTargetSet")}
                    </Button>
                  )}
                </div>
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
                      <FieldHelp text={t("playbooks.page.help.priority")} />
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
