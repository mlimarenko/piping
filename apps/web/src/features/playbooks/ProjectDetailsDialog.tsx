import { useTranslation } from "react-i18next";
import { useNavigate } from "react-router";
import { FileCode2, Rocket } from "lucide-react";

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
import type {
  PlaybookExecutionEnvironment,
  PlaybookProject,
} from "@/features/playbooks/api";

import { DetailField } from "./PlaybookFormFields";
import { formatDateTime } from "./playbookPageFormat";
import { projectFilesHref } from "./projectRoutes";

export function ProjectDetailsDialog({
  project,
  executionEnvironment,
  onOpenChange,
  onCreateJobTemplate,
  onCloseAutoFocus,
  onNavigate,
}: Readonly<{
  project: PlaybookProject | null;
  executionEnvironment?: PlaybookExecutionEnvironment;
  onOpenChange: (open: boolean) => void;
  onCreateJobTemplate: (project: PlaybookProject) => void;
  onCloseAutoFocus: (event: Event) => void;
  onNavigate: () => void;
}>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const navigate = useNavigate();
  if (!project) return null;

  return (
    <Dialog open={true} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-2xl" onCloseAutoFocus={onCloseAutoFocus}>
        <DialogHeader>
          <DialogTitle>{project.name}</DialogTitle>
          <DialogDescription>
            {project.description || t("playbooks.page.projects.noDescription")}
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3 py-2 md:grid-cols-2">
          <DetailField label={t("playbooks.page.projects.scmTypeLabel")}>
            <Badge variant="secondary" className="rounded-md">
              {t(`playbooks.page.projects.scmType.${project.scm_type}`, project.scm_type)}
            </Badge>
          </DetailField>
          <DetailField label={t("playbooks.page.projects.syncStatusLabel")}>
            <Badge
              variant={project.sync_status === "failed" ? "status-failed" : "status-neutral"}
              className="rounded-md"
            >
              {t(`playbooks.page.projects.syncStatus.${project.sync_status}`, project.sync_status)}
            </Badge>
          </DetailField>
          <DetailField label={t("playbooks.page.projects.source")} className="md:col-span-2" mono>
            {project.scm_url ?? t("playbooks.page.projects.manualSource")}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.basePath")} mono>
            {project.playbook_base_path || "—"}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.branch")} mono>
            {project.scm_branch || "—"}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.ref")} mono>
            {project.scm_ref || "—"}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.executionEnvironment")}>
            {executionEnvironment?.name ?? project.default_execution_environment_id ?? "—"}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.syncPolicy")}>
            {t(
              `playbooks.page.projects.syncPolicyValue.${project.sync_policy}`,
              project.sync_policy,
            )}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.lastRevision")} mono>
            {project.last_sync_revision || "—"}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.lastSynced")}>
            {formatDateTime(project.last_synced_at)}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.updated")}>
            {formatDateTime(project.updated_at)}
          </DetailField>
          <DetailField label={t("playbooks.page.fields.id")} className="md:col-span-2" mono>
            {project.id}
          </DetailField>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            {t("close")}
          </Button>
          <Button
            variant="outline"
            onClick={() => {
              onNavigate();
              onOpenChange(false);
              void navigate(projectFilesHref(project.id));
            }}
          >
            <FileCode2 className="h-3.5 w-3.5" />
            {t("playbooks.page.projects.files.action", { defaultValue: "Files" })}
          </Button>
          {can("CREATE", "PLAYBOOK_JOB_TEMPLATE") && (
            <Button onClick={() => onCreateJobTemplate(project)}>
              <Rocket className="h-3.5 w-3.5" />
              {t("playbooks.page.actions.newJobTemplate")}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
