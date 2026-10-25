import { useTranslation } from "react-i18next";
import { ChevronRight, ShieldAlert } from "lucide-react";

import EntityEditorPage from "@/shared/ui-shared/EntityEditorPage";
import { Button } from "@/shared/ui-primitives/button";
import { Input } from "@/shared/ui-primitives/input";
import { FormField } from "./PlaybookFormFields";

export function ServicePrincipalEditor({
  name,
  onNameChange,
  canReadPolicy,
  onGrantAccess,
  onSubmit,
  submitting,
  onBack,
  dirty = false,
}: Readonly<{
  name: string;
  onNameChange: (value: string) => void;
  canReadPolicy: boolean;
  onGrantAccess: () => void;
  onSubmit: () => void;
  submitting: boolean;
  onBack: () => void;
  dirty?: boolean;
}>) {
  const { t } = useTranslation();
  return (
    <EntityEditorPage scope="routed"
      title={t("playbooks.page.servicePrincipals.createTitle")}
      description={t("playbooks.page.servicePrincipals.createDescription")}
      onBack={() => { if (!submitting) onBack(); }}
      backLabel={t("back")}
      onSave={onSubmit}
      saveLabel={t("create")}
      saving={submitting}
      saveDisabled={!name.trim()}
      onCancel={onBack}
      cancelLabel={t("cancel")}
      dirty={dirty}
      dirtyMessage={t("editor.unsavedChanges", { defaultValue: "You have unsaved changes" })}
    >
      <fieldset disabled={submitting} inert={submitting} className="flex min-w-0 flex-col gap-4">
        <FormField label={t("playbooks.page.fields.servicePrincipalName")}>
          <Input
            className="h-9 text-sm"
            value={name}
            onChange={(event) => { if (!submitting) onNameChange(event.target.value); }}
            placeholder={t("playbooks.page.placeholders.servicePrincipalName")}
          />
        </FormField>
        {/* A fresh service principal has no permissions; it gets them only
            from an access policy whose subject is this principal. */}
        <div className="flex items-start gap-2 rounded-md border border-border bg-background px-3 py-2">
          <ShieldAlert className="mt-0.5 h-3.5 w-3.5 shrink-0 text-muted-foreground" />
          <div className="min-w-0 space-y-1">
            <p className="text-xs text-muted-foreground">
              {t("playbooks.page.servicePrincipals.createNoPermissionsCue")}
            </p>
            {canReadPolicy && (
              <Button variant="link" size="sm" className="px-0" onClick={onGrantAccess}>
                {t("playbooks.page.servicePrincipals.grantAccessOnAccessPage")}
                <ChevronRight className="ml-0.5 h-3 w-3" />
              </Button>
            )}
          </div>
        </div>
      </fieldset>
    </EntityEditorPage>
  );
}
