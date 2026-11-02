import { useTranslation } from "react-i18next";
import { KeyRound } from "lucide-react";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import FormSelect from "@/shared/ui-shared/FormSelect";
import SearchableSelect from "@/shared/ui-shared/SearchableSelect";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type {
  PlaybookCatalogItem,
  PlaybookScheduleConcurrencyPolicy,
  PlaybookServicePrincipal,
  PlaybookTargetSet,
} from "@/features/playbooks/api";

import { FormField, ToggleField } from "./PlaybookFormFields";
import { targetSetLabel } from "./playbookPageFormat";
import { concurrencyPolicies, type ScheduleForm } from "./playbookFormModels";

export function ScheduleEditor({
  editing,
  form,
  errors,
  playbooks,
  targetSets,
  servicePrincipals,
  onFormChange: changeDraft,
  spCreating,
  spName,
  onSpCreatingChange,
  onSpNameChange,
  onSpSubmit,
  spSubmitting,
  onBack,
  onSubmit,
  submitting,
  dirty = false,
}: Readonly<{
  editing: boolean;
  form: ScheduleForm;
  errors: Record<string, string>;
  playbooks: PlaybookCatalogItem[];
  targetSets: PlaybookTargetSet[];
  servicePrincipals: PlaybookServicePrincipal[];
  onFormChange: React.Dispatch<React.SetStateAction<ScheduleForm>>;
  spCreating: boolean;
  spName: string;
  onSpCreatingChange: (value: boolean) => void;
  onSpNameChange: (value: string) => void;
  onSpSubmit: () => void;
  spSubmitting: boolean;
  onBack: () => void;
  onSubmit: () => void;
  submitting: boolean;
  dirty?: boolean;
}>) {
  const { t } = useTranslation();
  const locked = submitting || spSubmitting;
  const onFormChange: typeof changeDraft = (next) => { if (!locked) changeDraft(next); };
  // View-but-not-edit: the inline "create service principal" opener is a real
  // CREATE entry point even from inside the schedule editor.
  const { can } = usePermissions();
  const canCreateServicePrincipal = can("CREATE", "PLAYBOOK_SERVICE_PRINCIPAL");
  const noServicePrincipals = servicePrincipals.length === 0;
  return (
    <EntityEditorPage scope="routed"
      title={
        editing
          ? t("playbooks.page.dialogs.schedule.editTitle")
          : t("playbooks.page.dialogs.schedule.title")
      }
      description={t("playbooks.page.dialogs.schedule.description")}
      onBack={() => { if (!locked) onBack(); }}
      backLabel={t("back")}
      onSave={onSubmit}
      saveLabel={editing ? t("save") : t("create")}
      saving={locked}
      onCancel={onBack}
      cancelLabel={t("cancel")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
    >
      <fieldset disabled={submitting || spSubmitting} inert={submitting || spSubmitting} className="grid min-w-0 gap-4 md:grid-cols-2">
        <FormField label={t("name")} className="md:col-span-2">
          <Input
            className="h-9 text-sm"
            value={form.name}
            onChange={(event) => onFormChange((prev) => ({ ...prev, name: event.target.value }))}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.playbook")}>
          <SearchableSelect
            value={form.playbookId}
            onValueChange={(value) => onFormChange((prev) => ({ ...prev, playbookId: value }))}
            disabled={locked || editing}
            items={playbooks.map((item) => ({
              id: item.playbook.id,
              label: item.playbook.name,
              searchText: item.playbook.slug,
            }))}
            placeholder={t("playbooks.page.placeholders.playbook")}
            searchPlaceholder={t("search")}
          />
        </FormField>
        <FormField label={t("playbooks.page.fields.targetSet")}>
          <SearchableSelect
            value={form.targetSetId}
            onValueChange={(value) => onFormChange((prev) => ({ ...prev, targetSetId: value }))}
            disabled={locked || targetSets.length === 0}
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
        <FormField label={t("playbooks.page.schedules.cron")}>
          <Input
            className="h-9 font-mono text-sm"
            value={form.cronExpression}
            onChange={(event) =>
              onFormChange((prev) => ({ ...prev, cronExpression: event.target.value }))
            }
            placeholder="0 3 * * *"
          />
          {errors.cron_expression ? (
            <p className="text-xs text-status-warning">{errors.cron_expression}</p>
          ) : null}
        </FormField>
        <FormField label={t("playbooks.page.schedules.timezone")}>
          <Input
            className="h-9 text-sm"
            value={form.timezone}
            onChange={(event) => onFormChange((prev) => ({ ...prev, timezone: event.target.value }))}
            placeholder="UTC"
          />
          {errors.timezone ? (
            <p className="text-xs text-status-warning">{errors.timezone}</p>
          ) : null}
        </FormField>
        <FormField label={t("playbooks.page.schedules.concurrency")}>
          <FormSelect<PlaybookScheduleConcurrencyPolicy>
            value={form.concurrencyPolicy}
            onValueChange={(concurrencyPolicy) =>
              onFormChange((prev) => ({ ...prev, concurrencyPolicy }))
            }
            options={concurrencyPolicies.map((policy) => ({
              value: policy,
              label: t(`playbooks.page.schedules.concurrencyPolicy.${policy}`),
            }))}
          />
        </FormField>
        {/* Misfire is fixed to the backend's single supported policy. */}
        <FormField label={t("playbooks.page.schedules.misfire")}>
          <Input
            className="h-9 text-sm"
            value={t("playbooks.page.schedules.misfirePolicy.fire_once")}
            readOnly
          />
        </FormField>
        {/* Service principal is set once at creation; editing keeps the bound one. */}
        {editing ? null : (
          <FormField label={t("playbooks.page.fields.servicePrincipal")} className="md:col-span-2">
            {spCreating ? (
              <div className="flex items-center gap-1.5">
                <Input
                  className="h-9 text-sm"
                  value={spName}
                  autoFocus
                  onChange={(event) => onSpNameChange(event.target.value)}
                  placeholder={t("playbooks.page.placeholders.servicePrincipalName")}
                />
                <Button
                  type="button"
                  size="sm"
                  className="h-9 text-sm"
                  disabled={spSubmitting || !spName.trim()}
                  onClick={onSpSubmit}
                >
                  {t("create")}
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-9 text-sm"
                  onClick={() => onSpCreatingChange(false)}
                >
                  {t("cancel")}
                </Button>
              </div>
            ) : (
              <div className="space-y-1.5">
                <SearchableSelect
                  value={form.servicePrincipalId}
                  onValueChange={(value) =>
                    onFormChange((prev) => ({ ...prev, servicePrincipalId: value }))
                  }
                  disabled={noServicePrincipals}
                  items={servicePrincipals.map((principal) => ({
                    id: principal.id,
                    label: principal.name,
                  }))}
                  placeholder={t(
                    noServicePrincipals
                      ? "playbooks.page.placeholders.noServicePrincipals"
                      : "playbooks.page.placeholders.servicePrincipal",
                  )}
                  searchPlaceholder={t("search")}
                />
                {canCreateServicePrincipal && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    className="gap-2 px-2 text-primary hover:text-primary"
                    onClick={() => onSpCreatingChange(true)}
                  >
                    <KeyRound className="h-3 w-3" />
                    {t("playbooks.page.actions.newServicePrincipal")}
                  </Button>
                )}
              </div>
            )}
          </FormField>
        )}
        <ToggleField
          label={t("enabled")}
          checked={form.enabled}
          onCheckedChange={(checked) => onFormChange((prev) => ({ ...prev, enabled: checked }))}
        />
      </fieldset>
    </EntityEditorPage>
  );
}
