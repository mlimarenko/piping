import { useTranslation } from "react-i18next";
import { Rocket } from "lucide-react";

import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/shared/ui-primitives/dialog";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { PlaybookExecutionEnvironment } from "@/features/playbooks/api";

import { DetailField } from "./PlaybookFormFields";
import { formatDateTime, jsonObjectSize } from "./playbookPageFormat";

export function ExecutionEnvironmentDetailsDialog({
  environment,
  onOpenChange,
  onCreateJobTemplate,
  onCloseAutoFocus,
}: Readonly<{
  environment: PlaybookExecutionEnvironment | null;
  onOpenChange: (open: boolean) => void;
  onCreateJobTemplate: (environment: PlaybookExecutionEnvironment) => void;
  onCloseAutoFocus: (event: Event) => void;
}>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  if (!environment) return null;

  const envCount = jsonObjectSize(environment.env);
  const settingsCount = jsonObjectSize(environment.settings);

  return (
    <Dialog open={true} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-2xl" onCloseAutoFocus={onCloseAutoFocus}>
        <DialogHeader>
          <DialogTitle>{environment.name}</DialogTitle>
          <DialogDescription>
            {environment.description ||
              t("playbooks.page.executionEnvironments.noDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3 py-2 md:grid-cols-2">
          <DetailField label={t("enabled")}>
            <Badge
              variant={environment.enabled ? "status-active" : "status-neutral"}
              className="rounded-md"
            >
              {environment.enabled ? t("enabled") : t("disabled")}
            </Badge>
          </DetailField>
          <DetailField label={t("playbooks.page.executionEnvironments.pullPolicy")}>
            {t(
              `playbooks.page.executionEnvironments.pullPolicies.${environment.pull_policy}`,
              environment.pull_policy,
            )}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.image")} className="md:col-span-2" mono>
            {environment.image}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.runnerGroup")} mono>
            {environment.runner_group}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.env")}>
            {t("playbooks.page.objectKeysCount", { count: envCount })}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.settings")}>
            {t("playbooks.page.objectKeysCount", { count: settingsCount })}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.created")}>
            {formatDateTime(environment.created_at)}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.updated")}>
            {formatDateTime(environment.updated_at)}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.id")} className="md:col-span-2" mono>
            {environment.id}
          </DetailField>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("close")}
          </Button>
          {can("CREATE", "PLAYBOOK_JOB_TEMPLATE") && (
            <Button onClick={() => onCreateJobTemplate(environment)}>
              <Rocket className="h-3.5 w-3.5" />
              {t("playbooks.page.actions.newJobTemplate")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
