import { useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Button } from "@/shared/ui-primitives/button";
import { Sheet, SheetTrigger, SheetContent, SheetHeader, SheetTitle, SheetDescription } from "@/shared/ui-primitives/sheet";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Input } from "@/shared/ui-primitives/input";
import type { SelectorFacet, SelectorModel } from "./selectorModel";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { TARGET_SET_TEMPLATES } from "./targetSetTemplates";
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
  memberLabelWarning = false,
  onBack,
  onSubmit,
  submitting,
  dirty = false,
  preview,
  previewPending = false,
  templateKey,
  onApplyTemplate,
  focusFacet,
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
  memberLabelWarning?: boolean;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
  preview?: ReactNode;
  previewPending?: boolean;
  templateKey?: string;
  onApplyTemplate?: (key: string) => void;
  focusFacet?: SelectorFacet;
}>) {
  const { t } = useTranslation();
  const [previewOpen, setPreviewOpen] = useState(false);
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
      saveDisabled={mode === "dynamic" && previewPending}
      onCancel={onBack}
      cancelLabel={t("cancel")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
    >
      {memberLabelWarning && <p role="status" className="text-sm text-muted-foreground">{t("playbooks.page.selectorBuilder.memberLabelsUnavailable")}</p>}
      {mode === "dynamic" && preview && <Sheet open={previewOpen} onOpenChange={setPreviewOpen}>
        <SheetTrigger asChild><Button type="button" variant="outline" className="md:hidden">{t("playbooks.selectorPreview.title")}</Button></SheetTrigger>
        <SheetContent side="right" className="w-full overflow-y-auto sm:max-w-lg"><SheetHeader><SheetTitle>{t("playbooks.selectorPreview.title")}</SheetTitle><SheetDescription>{t("playbooks.selectorPreview.description")}</SheetDescription></SheetHeader><div className="mt-4">{preview}</div></SheetContent>
      </Sheet>}
      <div className={mode === "dynamic" && preview ? "grid min-w-0 items-start gap-4 md:grid-cols-2" : "min-w-0"}>
      <fieldset disabled={submitting} inert={submitting} className="flex min-w-0 flex-col gap-4">
        {!editing && onApplyTemplate && <FormField label={t("playbooks.targetOverrides.template")}><SearchableSelect value={templateKey ?? ""} onValueChange={onApplyTemplate} placeholder={t("playbooks.targetOverrides.chooseTemplate")} items={TARGET_SET_TEMPLATES.map(template => ({ id: template.key, label: t(`playbooks.page.targetSetTemplates.${template.key}.title`) }))} /></FormField>}
        {templateKey === "linuxVm" && <p className="text-sm text-muted-foreground">{t("playbooks.targetOverrides.chooseLinuxPlatform")}</p>}
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
            focusFacet={focusFacet}
            onChange={(next) => { if (!submitting) onSelectorModelChange(next); }}
          />
        ) : (
          <HostMultiSelectPicker
            selected={staticHosts}
            onChange={(next) => { if (!submitting) onStaticHostsChange(next); }}
          />
        )}
      </fieldset>
      {mode === "dynamic" && preview && <>
        <aside className="hidden max-h-[calc(100dvh-10rem)] min-w-0 overflow-y-auto rounded-md border border-border p-4 md:block md:sticky md:top-4" aria-label={t("playbooks.selectorPreview.title")}><h2 className="mb-3 text-sm font-semibold">{t("playbooks.selectorPreview.title")}</h2>{preview}</aside>
      </>}
      </div>
    </EntityEditorPage>
  );
}
