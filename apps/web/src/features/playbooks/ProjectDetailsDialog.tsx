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
  Project,
} from "@/features/playbooks/api";

import { DetailField } from "./PlaybookFormFields";
import { formatDateTime } from "./playbookPageFormat";
import { projectFilesHref } from "./projectRoutes";

export function ProjectDetailsDialog({
  project,
  onOpenChange,
  onCreateJobTemplate,
  onCloseAutoFocus,
  onNavigate,
}: Readonly<{
  project: Project | null;
  executionEnvironment?: PlaybookExecutionEnvironment;
  onOpenChange: (open: boolean) => void;
  onCreateJobTemplate: (project: Project) => void;
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
              {project.source.kind === "editable" ? t("playbooks.page.projects.scmType.manual") : "Git"}
            </Badge>
          </DetailField>
          <DetailField label={t("playbooks.page.projects.source")} className="md:col-span-2" mono>
            {project.source.kind === "git" ? project.source.url : t("playbooks.page.projects.manualSource")}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.ref")} mono>
            {project.source.kind === "git" ? project.source.reference : "—"}
          </DetailField>
          <DetailField label={t("playbooks.page.projects.lastRevision")} mono>
            {project.current_revision_id || "—"}
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
