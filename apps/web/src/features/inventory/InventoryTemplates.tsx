import { useState } from "react";
import { useTranslation, Trans } from "react-i18next";
import { useNavigate } from "react-router";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiMutation } from "@/shared/api/hooks";
import { usePermissions } from "@/shared/auth/permissionsContext";
import PageChrome from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { useInventoryRowActionFocus } from "./useInventoryRowActionFocus";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import FormSection from "@/shared/ui-shared/FormSection";
import DataTable, { type Column } from "@/shared/ui-shared/DataTable";
import { useServerTable } from "@/shared/ui-shared/useServerTable";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import { Badge } from "@/shared/ui-primitives/badge";
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle } from "@/shared/ui-primitives/alert-dialog";
import { Plus, Pencil, Play, Trash2, FileText, Copy, ChevronDown, ChevronRight } from "lucide-react";
import { notify } from "@/shared/lib/notifications";
import TemplateRunDialog from "@/features/inventory/components/inventory/TemplateRunDialog";
import type { InventoryTemplate } from "@/features/inventory/components/inventory/templateTypes";
import { formatRelativeTime } from "@/shared/lib/format";
import {
  createTemplate as apiCreateTemplate,
  deleteTemplate as apiDeleteTemplate,
  listTemplates as apiListTemplates,
  runTemplate as apiRunTemplate,
  listTemplateRuns as apiListTemplateRuns,
  listRunSteps as apiListRunSteps,
} from "@/features/inventory/api";
import type { TemplateListQuery, TemplateRunListQuery } from "@/features/inventory/api";
import type { ApiError } from "@/shared/api/client";
import type { TemplateRunResponse, TemplateRunStep } from "@/shared/api/types";
import {
  INVENTORY_TEMPLATES_TABLE_CONFIG,
  INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG,
} from "@/features/inventory/templatesTableConfig";

const statusMap: Record<string, "positive" | "info" | "warning" | "neutral"> = {
  active: "positive", draft: "warning", archived: "neutral",
};

const runStatusMap = (status: string): "positive" | "info" | "warning" | "negative" | "neutral" => {
  switch (status) {
    case "completed": return "positive";
    case "completed_with_errors": return "warning";
    case "failed": return "negative";
    case "running": return "info";
    case "pending": return "info";
    default: return "neutral";
  }
};

const stepStatusMap = (status: string): "positive" | "info" | "warning" | "negative" | "neutral" => {
  switch (status) {
    case "success": return "positive";
    case "failed": return "negative";
    case "skipped": return "warning";
    case "running": return "info";
    default: return "neutral";
  }
};

function formatDuration(startedAt?: string, finishedAt?: string): string {
  if (!startedAt || !finishedAt) return "—";
  const ms = new Date(finishedAt).getTime() - new Date(startedAt).getTime();
  if (ms < 1000) return `${ms}ms`;
  return `${(ms / 1000).toFixed(1)}s`;
}


const TEMPLATES_BASE_KEY = ["inventory", "templates"] as const;
const ALL_RUNS_QUERY_KEY = ["inventory", "template-runs", "all"] as const;

// ── Recent Runs section ──────────────────────────────────────────────────────

type RecentRun = TemplateRunResponse & { templateName: string };

interface RecentRunsSectionProps {
  templates: InventoryTemplate[];
}

const RUNS_PAGE_SIZE = 8;

// Step-details drill-down for the currently expanded run. Lazily fetches steps
// for one run at a time (the row a user expanded) and renders below the table,
// preserving the original per-row expansion behaviour without a sub-row (the
// shared DataTable renders exactly one row per item).
const RunStepsDetail = ({ runId }: { runId: string }) => {
  const { t } = useTranslation();
  const stepsQuery = useQuery({
    queryKey: ["inventory", "run-steps", runId],
    queryFn: () => apiListRunSteps(runId),
  });
  const steps: TemplateRunStep[] = stepsQuery.data ?? [];

  if (stepsQuery.isLoading) {
    return <div className="px-4 py-2 text-xs text-muted-foreground">{t("loading")}…</div>;
  }
  if (steps.length === 0) {
    return (
      <div className="px-4 py-2 text-xs text-muted-foreground">
        {t("inventory.templates.recentRuns.noSteps")}
      </div>
    );
  }
  return (
    <div className="space-y-1 px-4 py-2">
      {steps.map((step) => (
        <div key={step.id} className="flex items-start gap-2 text-xs surface-inset rounded p-1.5">
          <StatusChip status={stepStatusMap(step.status)} label={step.status} className="shrink-0" />
          <span className="font-mono text-muted-foreground">{step.stepKey}</span>
          {step.durationMs !== undefined && (
            <span className="text-muted-foreground">{t("inventory.templates.recentRuns.durationMs", { ms: step.durationMs })}</span>
          )}
          {step.createdResourceIds.length > 0 && (
            <span className="text-status-positive">{t("inventory.templates.recentRuns.createdResources")} {step.createdResourceIds.length}</span>
          )}
          {step.errorMessage && (
            <span className="text-status-negative truncate max-w-xs">{step.errorMessage}</span>
          )}
        </div>
      ))}
    </div>
  );
};

const RecentRunsSection = ({ templates }: RecentRunsSectionProps) => {
  const { t } = useTranslation();
  const [expandedRunId, setExpandedRunId] = useState<string | null>(null);

  const table = useServerTable<TemplateRunResponse, TemplateRunListQuery>({
    listFn: apiListTemplateRuns,
    queryKey: ALL_RUNS_QUERY_KEY,
    sortKeys: INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG.sortKeys,
    filterColumns: INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG.filterColumns,
    pageSizeDefault: RUNS_PAGE_SIZE,
    keepPreviousData: true,
    staleTime: 5_000,
    refetchInterval: (query) => {
      const hasActive = query.state.data?.items.some(
        (run) => run.status === "pending" || run.status === "running",
      );
      return hasActive ? 2000 : false;
    },
  });
  const runsQuery = table.query;

  // Build per-template name lookup
  const templateNames: Record<string, string> = {};
  for (const tpl of templates) templateNames[tpl.id] = tpl.name;

  const runs: RecentRun[] = (runsQuery.data?.items ?? []).map((run) => ({
    ...run,
    templateName: run.templateName ?? templateNames[run.templateId] ?? run.templateId,
  }));
  const total = runsQuery.data?.total ?? 0;
  const pageRuns = runs;

  const expandedRun = pageRuns.find((run) => run.id === expandedRunId) ?? null;

  const goToPage = (next: number) => {
    table.setPage(next);
    setExpandedRunId(null);
  };

  const runStatusFilterOptions = [
    { value: "pending", label: t("inventory.templates.recentRuns.pending", "Pending") },
    { value: "running", label: t("inventory.templates.recentRuns.running", "Running") },
    { value: "completed", label: t("inventory.templates.recentRuns.completed", "Completed") },
    {
      value: "completed_with_errors",
      label: t("inventory.templates.recentRuns.completedWithErrors", "Completed with errors"),
    },
    { value: "failed", label: t("inventory.templates.recentRuns.failedStatus", "Failed") },
  ];
  const templateFilterOptions = templates.map((template) => ({
    value: template.id,
    label: template.name,
  }));

  const columns: Column<RecentRun>[] = [
    {
      key: "expand",
      label: "",
      excludeFromSettings: true,
      width: "2.5rem",
      render: (run) =>
        run.id === expandedRunId ? (
          <ChevronDown className="h-3 w-3 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3 w-3 text-muted-foreground" />
        ),
    },
    {
      key: "status",
      label: t("inventory.templates.recentRuns.status"),
      sortable: true,
      filterOptions: runStatusFilterOptions,
      render: (run) => {
        const isActive = run.status === "pending" || run.status === "running";
        return (
          <StatusChip
            status={runStatusMap(run.status)}
            label={t(`inventory.templates.recentRuns.${run.status === "completed_with_errors" ? "completedWithErrors" : run.status}`, run.status)}
            className={isActive ? "animate-pulse" : ""}
          />
        );
      },
    },
    {
      key: "template",
      label: t("inventory.templates.recentRuns.template"),
      sortable: true,
      filterOptions: templateFilterOptions,
      render: (run) => <span className="text-xs font-medium text-foreground">{run.templateName}</span>,
    },
    {
      key: "startedAt",
      label: t("inventory.templates.recentRuns.startedAt"),
      sortable: true,
      render: (run) => <span className="text-xs text-muted-foreground">{formatRelativeTime(run.startedAt)}</span>,
    },
    {
      key: "duration",
      label: t("inventory.templates.recentRuns.duration"),
      sortable: true,
      render: (run) => (
        <span className="font-mono text-xs text-muted-foreground">{formatDuration(run.startedAt, run.finishedAt)}</span>
      ),
    },
    {
      key: "steps",
      label: t("inventory.templates.recentRuns.steps"),
      sortable: true,
      getSortValue: (run) =>
        run.summary
          ? run.summary.succeeded_steps + run.summary.failed_steps + run.summary.skipped_steps
          : 0,
      render: (run) =>
        run.summary ? (
          <div className="flex gap-1">
            <Badge variant="outline" className="text-xs h-5 text-status-positive">{t("inventory.templates.recentRuns.succeeded", { count: run.summary.succeeded_steps })}</Badge>
            {run.summary.failed_steps > 0 && (
              <Badge variant="outline" className="text-xs h-5 text-status-negative">{t("inventory.templates.recentRuns.failed", { count: run.summary.failed_steps })}</Badge>
            )}
          </div>
        ) : (
          <span className="text-xs text-muted-foreground">—</span>
        ),
    },
  ];

  return (
    <FormSection title={t("inventory.templates.recentRuns.title")} action={total > 0 ? <Badge variant="outline" className="text-xs">{total}</Badge> : undefined} contentClassName="flex flex-col gap-4">

      <DataTable
        tableId="inventory.templateRuns"
        columns={columns}
        data={pageRuns}
        getRowKey={(run) => run.id}
        loading={runsQuery.isLoading}
        error={runsQuery.isError ? runsQuery.error : undefined}
        onRetry={table.refresh}
        selectedRow={expandedRun}
        sortState={table.sortState}
        onSortChange={(next) => {
          table.setSortState(next);
          setExpandedRunId(null);
        }}
        columnFilters={table.columnFilters}
        onColumnFilterChange={(key, values) => {
          table.setColumnFilter(key, values);
          setExpandedRunId(null);
        }}
        onRowClick={(run) =>
          setExpandedRunId((current) => (current === run.id ? null : run.id))
        }
        emptyState={
          <div className="p-8 text-center">
            <p className="text-sm text-muted-foreground">{t("inventory.templates.recentRuns.noRuns")}</p>
            <p className="text-xs text-muted-foreground mt-1">{t("inventory.templates.recentRuns.noRunsDescription")}</p>
          </div>
        }
        pagination={
          total > table.pageSize
            ? {
                page: table.page,
                pageSize: table.pageSize,
                total,
                pageSizeOptions: [8, 16, 32, 64],
                onPageChange: goToPage,
                onPageSizeChange: (size: number) => {
                  table.setPageSize(size);
                  setExpandedRunId(null);
                },
              }
            : undefined
        }
      />

      {expandedRun && (
        <div className="border-t border-border pt-4">
          <RunStepsDetail runId={expandedRun.id} />
        </div>
      )}
    </FormSection>
  );
};

// ── Main component ───────────────────────────────────────────────────────────

const InventoryTemplates = () => {
  const { triggerProps, restoreFocus } = useInventoryRowActionFocus();
  const { t } = useTranslation();
  const qc = useQueryClient();
  const navigate = useNavigate();

  const table = useServerTable<InventoryTemplate, TemplateListQuery>({
    listFn: apiListTemplates,
    queryKey: TEMPLATES_BASE_KEY,
    sortKeys: INVENTORY_TEMPLATES_TABLE_CONFIG.sortKeys,
    filterColumns: INVENTORY_TEMPLATES_TABLE_CONFIG.filterColumns,
    keepPreviousData: true,
  });
  const templatesQuery = table.query;
  const pageItems: InventoryTemplate[] = templatesQuery.data?.items ?? [];
  const totalTemplates = templatesQuery.data?.total ?? 0;

  // RecentRuns needs the full template list to fan out per-template run fetches
  // and build a global id→name lookup, independent of the table's page/search.
  const allTemplatesQuery = useQuery({
    queryKey: [...TEMPLATES_BASE_KEY, "all"],
    queryFn: () => apiListTemplates({ limit: 1000, offset: 0 }),
    placeholderData: (prev) => prev,
  });
  const allTemplates: InventoryTemplate[] = allTemplatesQuery.data?.items ?? [];

  const [runTarget, setRunTarget] = useState<InventoryTemplate | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<InventoryTemplate | null>(null);

  // View-but-not-edit: hide create/run/edit/duplicate/delete affordances the
  // caller can't use (template runs + duplicate are CREATE, per resource model).
  const { can } = usePermissions();
  const canCreate = can("CREATE", "INVENTORY");
  const canUpdate = can("UPDATE", "INVENTORY");
  const canDelete = can("DELETE", "INVENTORY");

  const invalidate = () => qc.invalidateQueries({ queryKey: TEMPLATES_BASE_KEY });
  const invalidateRuns = () => qc.invalidateQueries({ queryKey: ALL_RUNS_QUERY_KEY });

  // Duplicate is a one-shot clone (no editor round-trip); create/edit happen on
  // the dedicated `/inventory/templates/(new|:id/edit)` editor route.
  const createMutation = useApiMutation({
    mutationFn: (body: Omit<InventoryTemplate, "id" | "updatedAt">) => apiCreateTemplate(body),
    onSuccess: (created) => {
      void invalidate();
      notify("success", t("inventory.templates.templateSaved", { name: created.name }));
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const deleteMutation = useApiMutation({
    mutationFn: (id: string) => apiDeleteTemplate(id),
    onSuccess: () => {
      void invalidate();
      if (deleteTarget) {
        notify("success", t("inventory.templates.templateDeleted", { name: deleteTarget.name }));
      }
      setDeleteTarget(null);
    },
    onError: (err: ApiError) => notify("error", err.message || t("unknownError")),
  });

  const runMutation = useApiMutation({
    mutationFn: ({ id, variables }: { id: string; variables: Record<string, unknown> }) =>
      apiRunTemplate(id, variables),
    onSuccess: (run) => {
      void invalidate();
      void invalidateRuns();
      notify("success", t("inventory.templates.runStarted", { jobId: run.id }));
    },
    retry: false,
  });

  const openCreate = () => { void navigate("/inventory/templates/new"); };
  const openEdit = (tpl: InventoryTemplate) => { void navigate(`/inventory/templates/${tpl.id}/edit`); };

  const handleDelete = () => {
    if (!deleteTarget) return;
    deleteMutation.mutate(deleteTarget.id);
  };

  const handleDuplicate = (tpl: InventoryTemplate) => {
    const copy: Omit<InventoryTemplate, "id" | "updatedAt"> = {
      name: `${tpl.name} ${t("inventory.templates.copySuffix")}`,
      slug: `${tpl.slug}-copy`,
      status: "draft",
      description: tpl.description,
      variables: structuredClone(tpl.variables),
      steps: structuredClone(tpl.steps),
    };
    createMutation.mutate(copy);
  };

  const handleRun = async (variables: Record<string, unknown>) => {
    if (!runTarget) throw new Error("No run target");
    await runMutation.mutateAsync({ id: runTarget.id, variables });
  };

  const statusFilterOptions = [
    { value: "active", label: t("statuses.active", "Active") },
    { value: "draft", label: t("statuses.draft", "Draft") },
  ];

  const columns: Column<InventoryTemplate>[] = [
    { key: "name", label: t("name"), sortable: true, render: (r: InventoryTemplate) => (
      <div>
        <span className="font-medium text-foreground">{r.name}</span>
        {r.description && <p className="text-xs text-muted-foreground truncate max-w-[260px]">{r.description}</p>}
      </div>
    )},
    { key: "slug", label: t("proxyNodes.slug"), sortable: true, render: (r: InventoryTemplate) => <span className="font-mono text-xs text-muted-foreground">{r.slug}</span> },
    { key: "shape", label: t("inventory.templates.shape"), sortable: true, render: (r: InventoryTemplate) => (
      <div className="flex gap-1">
        <Badge variant="outline" className="text-xs h-5">{t("inventory.templates.varsCount", { count: r.variables.length })}</Badge>
        <Badge variant="outline" className="text-xs h-5">{t("inventory.templates.stepsCount", { count: r.steps.length })}</Badge>
      </div>
    )},
    { key: "status", label: t("status"), sortable: true, filterOptions: statusFilterOptions, render: (r: InventoryTemplate) => <StatusChip status={statusMap[r.status] ?? "neutral"} label={t(`statuses.${r.status}`, r.status)} /> },
    { key: "updatedAt", label: t("connections.columns.updated"), sortable: true },
    {
      key: "actions", label: "", excludeFromSettings: true, render: (r: InventoryTemplate) => (
        <div className="flex gap-1">
          {canCreate && <Button variant="ghost" size="icon-sm" className="text-status-positive" aria-label={t("inventory.templates.runTemplate")} title={t("inventory.templates.runTemplate")} onClick={(e) => { e.stopPropagation(); setRunTarget(r); }}><Play className="h-4 w-4" /></Button>}
          {(canUpdate || canCreate || canDelete) && <RowActions label={`${t("actions")}: ${r.name}`} {...triggerProps(r.id)}>
            {canUpdate && <DropdownMenuItem onSelect={() => openEdit(r)}><Pencil className="size-4" />{t("edit")}</DropdownMenuItem>}
            {canCreate && <DropdownMenuItem onSelect={() => handleDuplicate(r)}><Copy className="size-4" />{t("duplicate")}</DropdownMenuItem>}
            {canDelete && <DropdownMenuItem variant="destructive" onSelect={() => setDeleteTarget(r)}><Trash2 className="size-4" />{t("delete")}</DropdownMenuItem>}
          </RowActions>}
        </div>
      )
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <PageChrome
        title={t("inventory.templates.title")}
        icon={FileText}
        description={t("inventory.templates.description")}
        badge={t("inventory.templates.templatesCount", { count: allTemplates.length })}
        actions={canCreate ? <Button size="sm" className="h-9 text-sm gap-1" onClick={openCreate}><Plus className="h-3 w-3" /> {t("inventory.templates.createTemplate")}</Button> : undefined}
      />
      <DataTable
        tableId="inventory.templates"
        columns={columns}
        data={pageItems}
        getRowKey={r => r.id}
        onRowClick={(r) => setRunTarget(r)}
        loading={templatesQuery.isPending}
        error={templatesQuery.error}
        onRetry={table.refresh}
        pagination={{
          page: table.page,
          pageSize: table.pageSize,
          total: totalTemplates,
          onPageChange: table.setPage,
          onPageSizeChange: table.setPageSize,
        }}
        sortState={table.sortState}
        onSortChange={table.setSortState}
        columnFilters={table.columnFilters}
        onColumnFilterChange={table.setColumnFilter}
        emptyState={<EmptyState icon={FileText} title={t("inventory.templates.noTemplates")} description={t("inventory.templates.noTemplatesDescription")} actionLabel={canCreate ? t("inventory.templates.createTemplate") : undefined} onAction={canCreate ? openCreate : undefined} />}
        toolbar={
          <SearchInput
            value={table.search}
            onValueChange={table.setSearch}
            placeholder={t("inventory.templates.searchPlaceholder")}
            containerClassName="flex-1 max-w-xs"
          />
        }
      />

      <RecentRunsSection templates={allTemplates} />

      <TemplateRunDialog
        open={!!runTarget}
        onOpenChange={(o) => !o && setRunTarget(null)}
        template={runTarget}
        onRun={handleRun}
      />

      <AlertDialog open={!!deleteTarget} onOpenChange={open => !open && setDeleteTarget(null)}>
        <AlertDialogContent className="surface" onCloseAutoFocus={event => { event.preventDefault(); restoreFocus(); }}>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("inventory.templates.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              <Trans
                i18nKey="inventory.templates.deleteDescription"
                values={{ name: deleteTarget?.name ?? "" }}
                components={{ 1: <strong /> }}
              />
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete} className="bg-destructive text-destructive-foreground hover:bg-destructive/90">{t("delete")}</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
};

export default InventoryTemplates;
