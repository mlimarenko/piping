import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { Pencil, Play, Trash2 } from "lucide-react";

import DataTable, { type Column, type DataTableSortState } from "@/shared/ui-shared/DataTable";
import type { TablePaginationProps } from "@/shared/ui-shared/Pagination";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import { Switch } from "@/shared/ui-primitives/switch";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
} from "@/shared/ui-shared/tableState";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type {
  PlaybookCatalogItem,
  PlaybookSchedule,
  PlaybookServicePrincipal,
  PlaybookTargetSet,
} from "@/features/playbooks/api";

import { formatDateTime, targetSetLabel } from "./playbookPageFormat";
import { concurrencyPolicies } from "./playbookFormModels";
import PageState from "@/shared/ui-shared/PageState";
import { PLAYBOOK_SCHEDULES_TABLE_CONFIG } from "./tableConfig";

export function SchedulesPanel({
  parentScoped = false,
  schedules,
  loading,
  error,
  onRetry,
  sortState,
  onSortChange,
  columnFilters,
  onColumnFilterChange,
  pagination,
  playbookById,
  targetSetById,
  servicePrincipalById,
  onEdit,
  onToggle,
  onDelete,
  togglingId,
  onRunNow,
  runNowPending,
}: Readonly<{
  parentScoped?: boolean;
  schedules: PlaybookSchedule[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  sortState: DataTableSortState | null;
  onSortChange: (next: DataTableSortState | null) => void;
  columnFilters: Record<string, string[]>;
  onColumnFilterChange: (key: string, values: string[]) => void;
  pagination?: TablePaginationProps;
  playbookById: Map<string, PlaybookCatalogItem>;
  targetSetById: Map<string, PlaybookTargetSet>;
  servicePrincipalById: Map<string, PlaybookServicePrincipal>;
  onEdit: (schedule: PlaybookSchedule) => void;
  onToggle: (schedule: PlaybookSchedule, enabled: boolean) => void;
  onDelete: (schedule: PlaybookSchedule) => void;
  togglingId: string | null;
  onRunNow: (schedule: PlaybookSchedule) => void;
  runNowPending: boolean;
}>) {
  const { t } = useTranslation();
  // View-but-not-edit: gate schedule mutations; the enabled-toggle is an UPDATE,
  // run-now maps to EXECUTE playbook.
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "PLAYBOOK_SCHEDULE");
  const canDelete = can("DELETE", "PLAYBOOK_SCHEDULE");
  const canExecute = can("EXECUTE", "PLAYBOOK");
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, PLAYBOOK_SCHEDULES_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        PLAYBOOK_SCHEDULES_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const columns: Column<PlaybookSchedule>[] = [
    {
      key: "name",
      label: t("playbooks.page.fields.servicePrincipalName"),
      getSortValue: (schedule) => schedule.name,
      render: (schedule) => {
        const servicePrincipal = servicePrincipalById.get(schedule.service_principal_id);
        return (
          <div className="min-w-0">
            <span className="font-medium text-foreground">{schedule.name}</span>
            <div className="mt-0.5 flex flex-wrap gap-x-3 gap-y-1 text-xs text-muted-foreground">
              <span className="flex min-w-0 items-center gap-1">
                {t("playbooks.page.fields.servicePrincipal")}:{" "}
                <EntityRef name={servicePrincipal?.name} id={schedule.service_principal_id} />
              </span>
              <span>{t("playbooks.page.schedules.nextRun")}: {formatDateTime(schedule.next_run_at)}</span>
              <span>{t("playbooks.page.schedules.lastRun")}: {formatDateTime(schedule.last_run_at)}</span>
            </div>
          </div>
        );
      },
    },
    {
      key: "playbook",
      label: t("playbooks.page.fields.playbook"),
      filterOptions: parentScoped ? undefined : Array.from(playbookById.values()).map((item) => ({
        value: item.playbook.id,
        label: item.playbook.name,
      })),
      getSortValue: (schedule) =>
        playbookById.get(schedule.playbook_id)?.playbook.name ?? schedule.playbook_id,
      render: (schedule) => (
        <EntityRef
          name={playbookById.get(schedule.playbook_id)?.playbook.name}
          id={schedule.playbook_id}
        />
      ),
    },
    {
      key: "target_set",
      label: t("playbooks.page.fields.targetSet"),
      filterOptions: Array.from(targetSetById.values()).map((targetSet) => ({
        value: targetSet.id,
        label: targetSetLabel(targetSet),
      })),
      getSortValue: (schedule) =>
        targetSetById.get(schedule.target_set_id)?.name ?? schedule.target_set_id,
      render: (schedule) => (
        <span className="text-xs text-muted-foreground">
          {targetSetLabel(targetSetById.get(schedule.target_set_id))}
        </span>
      ),
    },
    {
      key: "cron",
      label: t("playbooks.page.schedules.cron"),
      getSortValue: (schedule) => schedule.cron_expression,
      render: (schedule) => (
        <Badge variant="secondary" className="rounded-md font-mono">
          {schedule.cron_expression}
        </Badge>
      ),
    },
    {
      key: "timezone",
      label: t("playbooks.page.schedules.timezone"),
      getSortValue: (schedule) => schedule.timezone,
      render: (schedule) => (
        <Badge variant="secondary" className="rounded-md">
          {schedule.timezone}
        </Badge>
      ),
    },
    {
      key: "concurrency",
      label: t("playbooks.page.schedules.concurrency"),
      filterOptions: concurrencyPolicies.map((policy) => ({
        value: policy,
        label: t(`playbooks.page.schedules.concurrencyPolicy.${policy}`),
      })),
      getSortValue: (schedule) => schedule.concurrency_policy,
      render: (schedule) => (
        <span className="text-xs text-muted-foreground">
          {t(`playbooks.page.schedules.concurrencyPolicy.${schedule.concurrency_policy}`)}
        </span>
      ),
    },
    {
      key: "enabled",
      label: t("enabled"),
      filterOptions: boolFilterOptions({
        trueLabel: t("enabled"),
        falseLabel: t("disabled"),
      }),
      getSortValue: (schedule) => schedule.enabled,
      render: (schedule) => (
        <div className="flex items-center gap-1.5">
          {canUpdate && (
            <Switch
              checked={schedule.enabled}
              disabled={togglingId === schedule.id}
              onCheckedChange={(checked) => onToggle(schedule, checked)}
              aria-label={t("playbooks.page.schedules.toggle")}
            />
          )}
          <span className="text-xs text-muted-foreground">
            {schedule.enabled ? t("enabled") : t("disabled")}
          </span>
        </div>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (schedule) => (
        <div className="flex items-center justify-end gap-2">
          {canExecute && (
            <Button
              variant="outline"
              size="sm"
              className="gap-2"
              disabled={runNowPending}
              onClick={() => onRunNow(schedule)}
              title={t("playbooks.page.schedules.runNow")}
              aria-label={t("playbooks.page.schedules.runNow")}
            >
              <Play className="h-3.5 w-3.5" />
            </Button>
          )}
          {(canUpdate || canDelete) && (
            <RowActions label={`${t("actions")}: ${schedule.name}`}>
              {canUpdate && (
                <DropdownMenuItem onSelect={() => window.setTimeout(() => onEdit(schedule), 180)}>
                  <Pencil className="size-4" />
                  {t("edit")}
                </DropdownMenuItem>
              )}
              {canUpdate && canDelete && <DropdownMenuSeparator />}
              {canDelete && (
                <DropdownMenuItem variant="destructive" onSelect={() => window.setTimeout(() => onDelete(schedule), 180)}>
                  <Trash2 className="size-4" />
                  {t("delete")}
                </DropdownMenuItem>
              )}
            </RowActions>
          )}
        </div>
      ),
    },
  ];

  return (
    <DataTable
      tableId="playbooks.schedules"
      className="min-h-0 flex-1"
      columns={columns}
      data={schedules}
      getRowKey={(schedule) => schedule.id}
      loading={loading}
      error={error}
      onRetry={onRetry}
      sortState={sanitizedSortState}
      onSortChange={onSortChange}
      columnFilters={sanitizedColumnFilters}
      onColumnFilterChange={onColumnFilterChange}
      pagination={pagination}
      emptyState={<PageState kind="empty" title={t("playbooks.page.empty.schedules")} />}
    />
  );
}
