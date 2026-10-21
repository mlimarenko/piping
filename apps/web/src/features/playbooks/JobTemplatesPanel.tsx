import { useTranslation } from "react-i18next";
import { FileCode2, GitBranch, Play } from "lucide-react";

import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type {
  PlaybookCatalogItem,
  PlaybookExecutionEnvironment,
  PlaybookJobTemplate,
  PlaybookProject,
  PlaybookTargetSet,
} from "@/features/playbooks/api";

import { jsonObjectSize } from "./playbookPageFormat";
import { ListState } from "./PlaybookPanelUi";

export function JobTemplatesPanel({
  jobTemplates,
  loading,
  error,
  onRetry,
  playbookById,
  projectById,
  targetSetById,
  executionEnvironmentById,
  onRun,
  runPending,
}: Readonly<{
  jobTemplates: PlaybookJobTemplate[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  playbookById: Map<string, PlaybookCatalogItem>;
  projectById: Map<string, PlaybookProject>;
  targetSetById: Map<string, PlaybookTargetSet>;
  executionEnvironmentById: Map<string, PlaybookExecutionEnvironment>;
  onRun: (template: PlaybookJobTemplate) => void;
  runPending: boolean;
}>) {
  const { t } = useTranslation();
  const { can } = usePermissions();
  const canExecute = can("EXECUTE", "PLAYBOOK");
  const columns: Column<PlaybookJobTemplate>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (template) => template.name,
      render: (template) => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-sm text-foreground">{template.name}</span>
            <Badge
              variant={template.enabled ? "status-active" : "status-neutral"}
              className="rounded-md"
            >
              {template.enabled ? t("enabled") : t("disabled")}
            </Badge>
            <Badge variant="secondary" className="rounded-md">
              {template.playbook_id
                ? t("playbooks.page.jobTemplates.sourcePlaybook")
                : t("playbooks.page.jobTemplates.sourceProject")}
            </Badge>
          </div>
          <p className="mt-0.5 line-clamp-1 text-xs text-muted-foreground">
            {template.description || t("playbooks.page.jobTemplates.noDescription")}
          </p>
        </div>
      ),
    },
    {
      key: "launch_plan",
      label: t("playbooks.page.jobTemplates.launchPlan"),
      getSortValue: (template) => {
        if (template.playbook_id) {
          return playbookById.get(template.playbook_id)?.playbook.name ?? template.playbook_id;
        }
        if (template.project_id) {
          return projectById.get(template.project_id)?.name ?? template.project_id;
        }
        return "";
      },
      render: (template) => {
        const targetSet = template.inventory_target_set_id
          ? targetSetById.get(template.inventory_target_set_id)
          : undefined;
        const projectSource = template.project_id ? (
          <span className="inline-flex min-w-0 items-center gap-1.5">
            <EntityRef
              name={projectById.get(template.project_id)?.name}
              id={template.project_id}
              icon={GitBranch}
            />
            {template.playbook_path ? (
              <span className="min-w-0 truncate font-mono text-xs text-muted-foreground">
                /{template.playbook_path}
              </span>
            ) : null}
          </span>
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        );
        const source = template.playbook_id ? (
          <EntityRef
            name={playbookById.get(template.playbook_id)?.playbook.name}
            id={template.playbook_id}
            icon={FileCode2}
          />
        ) : (
          projectSource
        );
        return (
          <div className="min-w-0 space-y-1">
            <div className="min-w-0 text-xs text-foreground">{source}</div>
            <div className="flex min-w-0 flex-wrap items-center gap-1.5">
              <Badge variant="secondary" className="rounded-md">
                {template.inventory_target_set_id
                  ? targetSet?.name ?? template.inventory_target_set_id
                  : t("playbooks.page.jobTemplates.noTargetSet")}
              </Badge>
              <Badge variant="secondary" className="rounded-md">
                {t("playbooks.page.jobTemplates.extraVarsCount", {
                  count: jsonObjectSize(template.extra_vars),
                })}
              </Badge>
              {template.ask_variables_on_launch ? (
                <Badge variant="status-neutral" className="rounded-md">
                  {t("playbooks.page.jobTemplates.survey")}
                </Badge>
              ) : null}
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
            disabled={runPending || !template.enabled}
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
      isRowClickable={(template) => template.enabled}
      loading={loading}
      error={error}
      onRetry={onRetry}
      emptyState={<ListState label={t("playbooks.page.empty.jobTemplates")} />}
    />
  );
}
