import { useTranslation } from "react-i18next";
import { useRef, useState } from "react";
import { FileCode2 } from "lucide-react";
import { Link } from "react-router";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import { ProjectDetailsDialog } from "./ProjectDetailsDialog";

import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { Badge } from "@/shared/ui-primitives/badge";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import EntityRef from "@/shared/ui-shared/EntityRef";
import type { PlaybookExecutionEnvironment, PlaybookProject } from "@/features/playbooks/api";

import { formatDateTime } from "./playbookPageFormat";
import { ListState, PreviewActions } from "./PlaybookPanelUi";
import { projectFilesHref } from "./projectRoutes";

export function ProjectsPanel({
  projects,
  loading,
  error,
  onRetry,
  executionEnvironmentById,
  onCreateJobTemplate,
}: Readonly<{
  projects: PlaybookProject[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  executionEnvironmentById: Map<string, PlaybookExecutionEnvironment>;
  onCreateJobTemplate: (project: PlaybookProject) => void;
}>) {
  const { t } = useTranslation();
  const anchor = useRef<HTMLElement>(null);
  const focus = useCollectionPreviewFocus(anchor);
  const [selected, setSelected] = useState<PlaybookProject | null>(null);
  const open = (project: PlaybookProject, trigger: HTMLElement) => {
    focus.capture(trigger);
    setSelected(project);
  };
  const columns: Column<PlaybookProject>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (project) => project.name,
      render: (project) => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-sm text-foreground">{project.name}</span>
            <Badge variant="secondary" className="rounded-md">
              {t(`playbooks.page.projects.scmType.${project.scm_type}`, project.scm_type)}
            </Badge>
            <Badge
              variant={project.sync_status === "failed" ? "status-failed" : "status-neutral"}
              className="rounded-md"
            >
              {t(
                `playbooks.page.projects.syncStatus.${project.sync_status}`,
                project.sync_status,
              )}
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
      getSortValue: (project) => project.scm_url ?? project.scm_type,
      render: (project) => (
        <div className="max-w-[28rem] truncate text-xs text-muted-foreground">
          {project.scm_url ?? t("playbooks.page.projects.manualSource")}
        </div>
      ),
    },
    {
      key: "base_path",
      label: t("playbooks.page.fields.basePath"),
      getSortValue: (project) => project.playbook_base_path ?? "",
      render: (project) => (
        <span className="font-mono text-xs text-muted-foreground">
          {project.playbook_base_path || "—"}
        </span>
      ),
    },
    {
      key: "execution_environment",
      label: t("playbooks.page.fields.executionEnvironment"),
      getSortValue: (project) =>
        project.default_execution_environment_id
          ? executionEnvironmentById.get(project.default_execution_environment_id)?.name ??
            project.default_execution_environment_id
          : "",
      render: (project) =>
        project.default_execution_environment_id ? (
          <EntityRef
            name={executionEnvironmentById.get(project.default_execution_environment_id)?.name}
            id={project.default_execution_environment_id}
          />
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
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
        <PreviewActions name={project.name} onOpen={trigger => open(project, trigger)}>
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
    <section ref={anchor} tabIndex={-1} aria-label={t("playbooks.page.projects.title")} className="flex min-h-0 flex-1 flex-col rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
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
      emptyState={<ListState label={t("playbooks.page.empty.projects")} />}
    />
    <ProjectDetailsDialog project={selected}
      executionEnvironment={selected?.default_execution_environment_id ? executionEnvironmentById.get(selected.default_execution_environment_id) : undefined}
      onCloseAutoFocus={focus.onCloseAutoFocus} onNavigate={focus.suppress}
      onOpenChange={value => { if (!value) setSelected(null); }}
      onCreateJobTemplate={project => { focus.suppress(); setSelected(null); onCreateJobTemplate(project); }} />
    </section>
  );
}
