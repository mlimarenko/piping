import { useMemo } from "react";
import { useTranslation } from "react-i18next";

import DataTable, { type Column, type DataTableSortState } from "@/shared/ui-shared/DataTable";
import type { TablePaginationProps } from "@/shared/ui-shared/Pagination";
import { Badge } from "@/shared/ui-primitives/badge";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
} from "@/shared/ui-shared/tableState";
import {
  isTerminalStatus,
  RUN_FILTER_STATUSES,
  runStatusLabelKey,
  runStatusVariant,
} from "@/features/playbooks/runPhase";
import type { PlaybookCatalogItem, PlaybookRunGroup } from "@/features/playbooks/api";

import { formatDateTime } from "./playbookPageFormat";
import PageState from "@/shared/ui-shared/PageState";
import { PLAYBOOK_RUN_GROUPS_TABLE_CONFIG } from "./tableConfig";

// Statuses offered in the run-history filter — 6 user-facing buckets, each
// 1:1 with a single API status value (no client-side union filtering needed).
const runStatuses = RUN_FILTER_STATUSES;

function formatDuration(startIso: string | null, endIso: string | null): string | null {
  if (!startIso) return null;
  const end = endIso ? new Date(endIso).getTime() : Date.now();
  const secs = Math.round((end - new Date(startIso).getTime()) / 1000);
  const m = Math.floor(secs / 60);
  const s = secs % 60;
  return m > 0 ? `${m}m ${s}s` : `${s}s`;
}

export function RunsPanel({
  parentScoped = false,
  runGroups,
  loading,
  error,
  onRetry,
  playbooks,
  playbookById,
  selectedGroupId,
  sortState,
  onSortChange,
  columnFilters,
  onColumnFilterChange,
  pagination,
  onOpenGroup,
}: Readonly<{
  parentScoped?: boolean;
  runGroups: PlaybookRunGroup[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  playbooks: PlaybookCatalogItem[];
  playbookById: Map<string, PlaybookCatalogItem>;
  selectedGroupId?: string | null;
  sortState: DataTableSortState | null;
  onSortChange: (next: DataTableSortState | null) => void;
  columnFilters: Record<string, string[]>;
  onColumnFilterChange: (key: string, values: string[]) => void;
  pagination?: TablePaginationProps;
  onOpenGroup: (groupId: string, playbookName: string) => void;
}>) {
  const { t } = useTranslation();
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, PLAYBOOK_RUN_GROUPS_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        PLAYBOOK_RUN_GROUPS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const columns: Column<PlaybookRunGroup>[] = [
    {
      key: "playbook",
      label: t("playbooks.page.fields.playbook"),
      filterOptions: parentScoped ? undefined : playbooks.map((item) => ({
        value: item.playbook.id,
        label: item.playbook.name,
      })),
      getSortValue: (group) =>
        playbookById.get(group.playbook_id)?.playbook.name ?? group.playbook_id,
      render: (group) => {
        const playbook = playbookById.get(group.playbook_id);
        const playbookName =
          playbook?.playbook.name ?? t("playbooks.runDetail.unknownPlaybook");
        return (
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-medium text-foreground">{playbookName}</span>
            </div>
            <div className="mt-1 flex flex-wrap gap-2 text-xs text-muted-foreground">
              <span>
                {t("playbooks.page.runs.queuedAt")}: {formatDateTime(group.created_at)}
              </span>
              {group.started_at ? (
                <span>
                  {t("playbooks.page.runs.startedAt")}: {formatDateTime(group.started_at)}
                </span>
              ) : null}
            </div>
          </div>
        );
      },
    },
    {
      key: "started_at",
      label: t("playbooks.page.runs.startedAt"),
      getSortValue: (group) => group.started_at ?? group.created_at,
      render: (group) => (
        <span className="text-xs text-muted-foreground">
          {formatDateTime(group.started_at ?? group.created_at)}
        </span>
      ),
    },
    {
      key: "status",
      label: t("status"),
      filterOptions: runStatuses.map((status) => ({
        value: status,
        label: t(runStatusLabelKey(status)),
      })),
      getSortValue: (group) => group.status,
      render: (group) => (
        <Badge variant={runStatusVariant(group.status)} className="rounded-md">
          {t(runStatusLabelKey(group.status))}
        </Badge>
      ),
    },
    {
      key: "duration",
      label: t("playbooks.page.runs.duration"),
      getSortValue: (group) =>
        group.started_at
          ? new Date(group.finished_at ?? new Date().toISOString()).getTime() -
            new Date(group.started_at).getTime()
          : 0,
      render: (group) => {
        const duration = formatDuration(group.started_at ?? null, group.finished_at ?? null);
        if (!duration) return <span className="text-xs text-muted-foreground">—</span>;
        return (
          <span className="text-xs text-muted-foreground">
            {isTerminalStatus(group.status)
              ? t("playbooks.page.runs.duration")
              : t("playbooks.page.runs.elapsed")}
            : {duration}
          </span>
        );
      },
    },
    {
      key: "total_runs",
      label: t("playbooks.page.fields.targets"),
      getSortValue: (group) => group.total_runs,
      render: (group) => (
        <span className="text-xs text-muted-foreground">
          {t("playbooks.page.runs.targetCount", { count: group.total_runs })}
        </span>
      ),
    },
  ];

  return (
    <DataTable
      tableId="playbooks.runs"
      className="min-h-0 flex-1"
      columns={columns}
      data={runGroups}
      getRowKey={(group) => group.id}
      selectedRow={runGroups.find((group) => group.id === selectedGroupId) ?? null}
      onRowClick={(group) => {
        const playbook = playbookById.get(group.playbook_id);
        const playbookName =
          playbook?.playbook.name ?? t("playbooks.runDetail.unknownPlaybook");
        onOpenGroup(group.id, playbookName);
      }}
      loading={loading}
      error={error}
      onRetry={onRetry}
      sortState={sanitizedSortState}
      onSortChange={onSortChange}
      columnFilters={sanitizedColumnFilters}
      onColumnFilterChange={onColumnFilterChange}
      pagination={pagination}
      emptyState={<PageState kind="empty" title={t("playbooks.page.empty.runs")} />}
    />
  );
}
