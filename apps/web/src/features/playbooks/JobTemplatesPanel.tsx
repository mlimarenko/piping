import { useTranslation } from "react-i18next";
import type { ReactNode } from "react";
import { GitBranch, Play } from "lucide-react";

import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type {
  PlaybookExecutionEnvironment,
  JobTemplate,
  Project,
  PlaybookTargetSet,
} from "@/features/playbooks/api";

import { jsonObjectSize } from "./playbookPageFormat";
import PageState from "@/shared/ui-shared/PageState";

function hasProjectEntrypoint(template: JobTemplate): boolean {
  return Boolean(template.project_id && template.playbook_paths.some((path) => path.trim()));
}

export function JobTemplatesPanel({
  jobTemplates,
  loading,
  error,
  onRetry,
  projectById,
  targetSetById,
  executionEnvironmentById,
  onRun,
  runPending,
  emptyState,
}: Readonly<{
  jobTemplates: JobTemplate[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  projectById: Map<string, Project>;
  targetSetById: Map<string, PlaybookTargetSet>;
  executionEnvironmentById: Map<string, PlaybookExecutionEnvironment>;
  onRun: (template: JobTemplate) => void;
  runPending: boolean;
  emptyState?: ReactNode;
}>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const canExecute = can("EXECUTE", "PLAYBOOK_JOB_TEMPLATE") && can("EXECUTE_NATIVE", "PLAYBOOK_JOB_TEMPLATE");
  const columns: Column<JobTemplate>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (template) => template.name,
      render: (template) => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-sm text-foreground">{template.name}</span>
            {!hasProjectEntrypoint(template) && <Badge variant="status-neutral" className="rounded-md">{t("playbooks.jobDraft.incomplete", { defaultValue: "Unavailable: project or entrypoint missing" })}</Badge>}
          </div>
        </div>
      ),
    },
    {
      key: "launch_plan",
      label: t("playbooks.page.jobTemplates.launchPlan"),
      getSortValue: (template) => {
        if (template.project_id) {
          return projectById.get(template.project_id)?.name ?? template.project_id;
        }
        return "";
      },
      render: (template) => {
        const project = projectById.get(template.project_id);
        const targetSetId = template.inventory.kind === "managed" ? template.inventory.target_set_id : null;
        const targetSet = targetSetId
          ? targetSetById.get(targetSetId)
          : undefined;
        const source = template.project_id ? (
          <span className="inline-flex min-w-0 items-center gap-1.5">
            <EntityRef
              name={projectById.get(template.project_id)?.name}
              id={template.project_id}
              icon={GitBranch}
            />
            {template.playbook_paths.length ? (
              <span className="min-w-0 truncate font-mono text-xs text-muted-foreground">
                /{template.playbook_paths.join(", ")}
              </span>
            ) : null}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        );
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <div className="min-w-0 text-xs text-foreground">{source}</div>
            {project?.source.kind === "git" && <span className="break-all text-xs text-muted-foreground">{template.revision.kind === "fixed" ? t("playbooks.gitSource.pin") : `${t("playbooks.gitSource.follow")}: ${project.source.reference}`}</span>}
            <div className="flex min-w-0 flex-wrap items-center gap-1.5">
              <Badge variant="secondary" className="rounded-md">
                {targetSetId ? targetSet?.name ?? targetSetId : t("playbooks.jobDraft.nativeInventory", { defaultValue: "Project inventory" })}
              </Badge>
              <Badge variant="secondary" className="rounded-md">
                {t("playbooks.page.jobTemplates.extraVarsCount", {
                  count: jsonObjectSize(template.options.extra_vars),
                })}
              </Badge>
            </div>
          </div>
        );
      },
    },
    {
      key: "execution_environment",
      label: t("playbooks.page.fields.executionEnvironment"),
      getSortValue: (template) =>
        template.execution_environment_id
          ? executionEnvironmentById.get(template.execution_environment_id)?.name ??
            template.execution_environment_id
          : "",
      render: (template) =>
        template.execution_environment_id ? (
          <EntityRef
            name={executionEnvironmentById.get(template.execution_environment_id)?.name}
            id={template.execution_environment_id}
          />
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (template) =>
        canExecute ? (
          <Button
            size="sm"
            className="gap-2"
            disabled={runPending || !hasProjectEntrypoint(template)}
            title={t("playbooks.page.actions.run")}
            onClick={(event) => {
              event.stopPropagation();
              onRun(template);
            }}
          >
            <Play className="h-3.5 w-3.5" />
            {t("playbooks.page.actions.run")}
          </Button>
        ) : null,
    },
  ];

  return (
    <DataTable
      tableId="playbooks.jobTemplates"
      className="min-h-0 flex-1"
      columns={columns}
      data={jobTemplates}
      getRowKey={(template) => template.id}
      onRowClick={canExecute && !runPending ? onRun : undefined}
      isRowClickable={hasProjectEntrypoint}
      loading={loading}
      error={error}
      onRetry={onRetry}
      emptyState={emptyState ?? <PageState kind="empty" title={t("playbooks.page.empty.jobTemplates")} />}
    />
  );
}
