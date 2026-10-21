import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Pencil, Trash2 } from "lucide-react";
import RowActions from "@/shared/ui-shared/RowActions";

import DataTable, { type Column, type DataTableSortState } from "@/shared/ui-shared/DataTable";
import type { TablePaginationProps } from "@/shared/ui-shared/Pagination";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
} from "@/shared/ui-shared/tableState";
import { Badge } from "@/shared/ui-primitives/badge";
import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/shared/ui-primitives/dropdown-menu";
import SelectorChips from "@/features/playbooks/SelectorChips";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { PlaybookTargetSet } from "@/features/playbooks/api";

import { formatDateTime } from "./playbookPageFormat";
import { ListState } from "./PlaybookPanelUi";
import { PLAYBOOK_TARGET_SETS_TABLE_CONFIG } from "./tableConfig";

export function TargetSetsPanel({
  targetSets,
  loading,
  error,
  onRetry,
  selectedId,
  sortState,
  onSortChange,
  columnFilters,
  onColumnFilterChange,
  pagination,
  onEdit,
  onDelete,
  onSelect,
}: Readonly<{
  targetSets: PlaybookTargetSet[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  selectedId: string | null;
  sortState: DataTableSortState | null;
  onSortChange: (next: DataTableSortState | null) => void;
  columnFilters: Record<string, string[]>;
  onColumnFilterChange: (key: string, values: string[]) => void;
  pagination?: TablePaginationProps;
  onEdit: (targetSet: PlaybookTargetSet) => void;
  onDelete: (targetSet: PlaybookTargetSet) => void;
  onSelect: (targetSet: PlaybookTargetSet) => void;
}>) {
  const { t } = useTranslation();
  // View-but-not-edit: gate target-set CRUD affordances.
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "PLAYBOOK_TARGET_SET");
  const canDelete = can("DELETE", "PLAYBOOK_TARGET_SET");
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, PLAYBOOK_TARGET_SETS_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        PLAYBOOK_TARGET_SETS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const columns: Column<PlaybookTargetSet>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (targetSet) => targetSet.name,
      render: (targetSet) => (
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-sm text-foreground">{targetSet.name}</span>
            <Badge
              variant={targetSet.mode === "static" ? "status-neutral" : "secondary"}
              className="rounded-md"
            >
              {t(`playbooks.page.targetSetMode.${targetSet.mode}`)}
            </Badge>
          </div>
          <SelectorChips selector={targetSet.selector} className="mt-1 block" />
        </div>
      ),
    },
    {
      key: "mode",
      label: t("playbooks.page.fields.mode"),
      sortable: true,
      filterOptions: ["dynamic", "static"].map((mode) => ({
        value: mode,
        label: t(`playbooks.page.targetSetMode.${mode}`),
      })),
      getSortValue: (targetSet) => targetSet.mode,
      render: (targetSet) => (
        <Badge
          variant={targetSet.mode === "static" ? "status-neutral" : "secondary"}
          className="rounded-md"
        >
          {t(`playbooks.page.targetSetMode.${targetSet.mode}`)}
        </Badge>
      ),
    },
    {
      key: "updated_at",
      label: t("playbooks.page.fields.updated"),
      getSortValue: (targetSet) => targetSet.updated_at,
      render: (targetSet) => (
        <span className="text-xs text-muted-foreground">
          {formatDateTime(targetSet.updated_at)}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (targetSet) =>
        canUpdate || canDelete ? (
          <TargetSetActions
            targetSet={targetSet}
            canUpdate={canUpdate}
            canDelete={canDelete}
            onEdit={onEdit}
            onDelete={onDelete}
          />
        ) : null,
    },
  ];

  return (
    <DataTable
      tableId="playbooks.targetSets"
      className="min-h-0 flex-1"
      columns={columns}
      data={targetSets}
      getRowKey={(targetSet) => targetSet.id}
      selectedRow={targetSets.find((targetSet) => targetSet.id === selectedId) ?? null}
      onRowClick={onSelect}
      loading={loading}
      error={error}
      onRetry={onRetry}
      sortState={sanitizedSortState}
      onSortChange={onSortChange}
      columnFilters={sanitizedColumnFilters}
      onColumnFilterChange={onColumnFilterChange}
      pagination={pagination}
      emptyState={<ListState label={t("playbooks.page.empty.targetSets")} />}
    />
  );
}

export function TargetSetActions({
  targetSet,
  canUpdate,
  canDelete,
  onEdit,
  onDelete,
}: Readonly<{
  targetSet: PlaybookTargetSet;
  canUpdate: boolean;
  canDelete: boolean;
  onEdit: (targetSet: PlaybookTargetSet) => void;
  onDelete: (targetSet: PlaybookTargetSet) => void;
}>) {
  const { t } = useTranslation();
  const [menuOpen, setMenuOpen] = useState(false);
  const closeMenuThen = (action: () => void) => {
    setMenuOpen(false);
    window.setTimeout(action, 180);
  };

  return (
    <RowActions label={`${t("actions")}: ${targetSet.name}`} open={menuOpen} onOpenChange={setMenuOpen}>
        {canUpdate && (
          <DropdownMenuItem
            onSelect={(event) => {
              event.preventDefault();
              closeMenuThen(() => onEdit(targetSet));
            }}
          >
            <Pencil className="h-3.5 w-3.5" />
            {t("playbooks.page.actions.editTargetSet")}
          </DropdownMenuItem>
        )}
        {canDelete && (
          <>
            {canUpdate && <DropdownMenuSeparator />}
            <DropdownMenuItem
              variant="destructive"
              onSelect={(event) => {
                event.preventDefault();
                closeMenuThen(() => onDelete(targetSet));
              }}
            >
              <Trash2 className="h-3.5 w-3.5" />
              {t("playbooks.page.actions.deleteTargetSet")}
            </DropdownMenuItem>
          </>
        )}
    </RowActions>
  );
}
