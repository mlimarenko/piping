import { useTranslation } from "react-i18next";
import { useRef, useState } from "react";
import { useCollectionPreviewFocus } from "@/shared/ui-shared/useCollectionPreviewFocus";
import { ExecutionEnvironmentDetailsDialog } from "./ExecutionEnvironmentDetailsDialog";

import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { Badge } from "@/shared/ui-primitives/badge";
import type { PlaybookExecutionEnvironment } from "@/features/playbooks/api";

import { formatDateTime } from "./playbookPageFormat";
import { PreviewActions } from "./PlaybookPanelUi";
import PageState from "@/shared/ui-shared/PageState";

export function ExecutionEnvironmentsPanel({
  executionEnvironments,
  loading,
  error,
  onRetry,
  onCreateJobTemplate,
}: Readonly<{
  executionEnvironments: PlaybookExecutionEnvironment[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  onCreateJobTemplate: (environment: PlaybookExecutionEnvironment) => void;
}>) {
  const { t } = useTranslation();
  const anchor = useRef<HTMLElement>(null);
  const focus = useCollectionPreviewFocus(anchor);
  const [selected, setSelected] = useState<PlaybookExecutionEnvironment | null>(null);
  const open = (environment: PlaybookExecutionEnvironment, trigger: HTMLElement) => {
    focus.capture(trigger);
    setSelected(environment);
  };
  const columns: Column<PlaybookExecutionEnvironment>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (environment) => environment.name,
      render: (environment) => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-sm text-foreground">{environment.name}</span>
            <Badge
              variant={environment.enabled ? "status-active" : "status-neutral"}
              className="rounded-md"
            >
              {environment.enabled ? t("enabled") : t("disabled")}
            </Badge>
          </div>
          <p className="mt-0.5 line-clamp-1 text-xs text-muted-foreground">
            {environment.description || t("playbooks.page.executionEnvironments.noDescription")}
          </p>
        </div>
      ),
    },
    {
      key: "image",
      label: t("playbooks.page.fields.image"),
      getSortValue: (environment) => environment.image,
      render: (environment) => (
        <Badge variant="secondary" className="rounded-md font-mono">
          {environment.image}
        </Badge>
      ),
    },
    {
      key: "pull_policy",
      label: t("playbooks.page.executionEnvironments.pullPolicy"),
      getSortValue: (environment) => environment.pull_policy,
      render: (environment) => (
        <span className="text-xs text-muted-foreground">
          {t(
            `playbooks.page.executionEnvironments.pullPolicies.${environment.pull_policy}`,
            environment.pull_policy,
          )}
        </span>
      ),
    },
    {
      key: "runner_group",
      label: t("playbooks.page.fields.runnerGroup"),
      getSortValue: (environment) => environment.runner_group,
      render: (environment) => (
        <span className="text-xs text-muted-foreground">{environment.runner_group}</span>
      ),
    },
    {
      key: "updated_at",
      label: t("playbooks.page.fields.updated"),
      getSortValue: (environment) => environment.updated_at,
      render: (environment) => (
        <span className="text-xs text-muted-foreground">
          {formatDateTime(environment.updated_at)}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (environment) => (
        <PreviewActions name={environment.name} onOpen={trigger => open(environment, trigger)} />
      ),
    },
  ];

  return (
    <section ref={anchor} tabIndex={-1} aria-label={t("playbooks.page.executionEnvironments.title")} className="flex min-h-0 flex-1 flex-col rounded-md focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring">
    <DataTable
      tableId="playbooks.executionEnvironments"
      className="min-h-0 flex-1"
      columns={columns}
      data={executionEnvironments}
      getRowKey={(environment) => environment.id}
      onRowClick={open}
      loading={loading}
      error={error}
      onRetry={onRetry}
      emptyState={<PageState kind="empty" title={t("playbooks.page.empty.executionEnvironments")} />}
    />
    <ExecutionEnvironmentDetailsDialog environment={selected} onCloseAutoFocus={focus.onCloseAutoFocus}
      onOpenChange={value => { if (!value) setSelected(null); }}
      onCreateJobTemplate={environment => { focus.suppress(); setSelected(null); onCreateJobTemplate(environment); }} />
    </section>
  );
}
