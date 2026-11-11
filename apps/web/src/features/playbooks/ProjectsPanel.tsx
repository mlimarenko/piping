import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import { FileCode2 } from "lucide-react";
import { Link, useNavigate } from "react-router";

import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { Badge } from "@/shared/ui-primitives/badge";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import type { PlaybookExecutionEnvironment, Project } from "@/features/playbooks/api";

import { formatDateTime } from "./playbookPageFormat";
import { PreviewActions } from "./PlaybookPanelUi";
import PageState from "@/shared/ui-shared/PageState";
import { projectFilesHref } from "./projectRoutes";

export function ProjectsPanel({
  projects,
  loading,
  error,
  onRetry,
  emptyState,
}: Readonly<{
  projects: Project[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  executionEnvironmentById: Map<string, PlaybookExecutionEnvironment>;
  onCreateJobTemplate: (project: Project) => void;
  emptyState?: ReactNode;
}>) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const open = (project: Project) => { void navigate(projectFilesHref(project.id)); };
  const columns: Column<Project>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (project) => project.name,
      render: (project) => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-sm text-foreground">{project.name}</span>
            <Badge variant="secondary" className="rounded-md">
              {project.source.kind === "editable" ? t("playbooks.page.projects.scmType.manual") : "Git"}
            </Badge>
          </div>
          <p className="mt-0.5 line-clamp-1 text-xs text-muted-foreground">
            {project.description || t("playbooks.page.projects.noDescription")}
          </p>
        </div>
      ),
    },
    {
      key: "source",
      label: t("playbooks.page.projects.source"),
      getSortValue: (project) => project.source.kind === "git" ? project.source.url : project.source.kind,
      render: (project) => (
        <div className="max-w-[28rem] truncate text-xs text-muted-foreground">
          {project.source.kind === "git" ? project.source.url : t("playbooks.page.projects.manualSource")}
        </div>
      ),
    },
    {
      key: "updated_at",
      label: t("playbooks.page.fields.updated"),
      getSortValue: (project) => project.updated_at,
      render: (project) => (
        <span className="text-xs text-muted-foreground">
          {formatDateTime(project.updated_at)}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (project) => (
        <PreviewActions name={project.name} onOpen={() => open(project)}>
          <DropdownMenuItem asChild>
            <Link to={projectFilesHref(project.id)}>
              <FileCode2 className="h-3.5 w-3.5" />
              {t("playbooks.page.projects.files.action", { defaultValue: "Files" })}
            </Link>
          </DropdownMenuItem>
        </PreviewActions>
      ),
    },
  ];

  return (
    <section tabIndex={-1} aria-label={t("playbooks.page.projects.title")} className="flex min-h-0 flex-1 flex-col rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
    <DataTable
      tableId="playbooks.projects"
      className="min-h-0 flex-1"
      columns={columns}
      data={projects}
      getRowKey={(project) => project.id}
      onRowClick={open}
      loading={loading}
      error={error}
      onRetry={onRetry}
      emptyState={emptyState ?? <PageState kind="empty" title={t("playbooks.page.empty.projects")} />}
    />
    </section>
  );
}
