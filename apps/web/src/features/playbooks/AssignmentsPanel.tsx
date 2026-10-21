import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Database, Pencil, Play, Trash2 } from "lucide-react";
import RowActions from "@/shared/ui-shared/RowActions";

import DataTable, { type Column, type DataTableSortState } from "@/shared/ui-shared/DataTable";
import type { TablePaginationProps } from "@/shared/ui-shared/Pagination";
import { Badge } from "@/shared/ui-primitives/badge";
import { Button } from "@/shared/ui-primitives/button";
import {
  DropdownMenuItem,
  DropdownMenuSeparator,
} from "@/shared/ui-primitives/dropdown-menu";
import EntityRef from "@/shared/ui-shared/EntityRef";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
} from "@/shared/ui-shared/tableState";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type {
  PlaybookAssignment,
  PlaybookCatalogItem,
  PlaybookTargetSet,
} from "@/features/playbooks/api";
import type { usePlaybookScopeEntities } from "@/features/playbooks/usePlaybookScopeEntities";

import { targetSetLabel } from "./playbookPageFormat";
import { ListState } from "./PlaybookPanelUi";
import { PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG } from "./tableConfig";

export function AssignmentsPanel({
  parentScoped = false,
  assignments,
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
  scopeEntities,
  onEdit,
  onDelete,
  onPreview,
  onRun,
  runPending,
}: Readonly<{
  parentScoped?: boolean;
  assignments: PlaybookAssignment[];
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
  scopeEntities: ReturnType<typeof usePlaybookScopeEntities>;
  onEdit: (assignment: PlaybookAssignment) => void;
  onDelete: (assignment: PlaybookAssignment) => void;
  onPreview: (assignment: PlaybookAssignment) => void;
  onRun: (assignment: PlaybookAssignment) => void;
  runPending: boolean;
}>) {
  const { t } = useTranslation();
  // View-but-not-edit: gate assignment mutations; run maps to EXECUTE playbook.
  const { can } = usePermissions();
  const canUpdate = can("UPDATE", "PLAYBOOK_ASSIGNMENT");
  const canDelete = can("DELETE", "PLAYBOOK_ASSIGNMENT");
  const canExecute = can("EXECUTE", "PLAYBOOK");
  const sanitizedSortState = useMemo(
    () => sanitizeDataTableSortState(sortState, PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.sortKeys),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const columns: Column<PlaybookAssignment>[] = [
    {
      key: "name",
      label: t("name"),
      getSortValue: (assignment) =>
        assignment.name ||
        playbookById.get(assignment.playbook_id)?.playbook.name ||
        assignment.id,
      render: (assignment) => {
        const playbook = playbookById.get(assignment.playbook_id);
        return (
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <span className="font-medium text-sm text-foreground">
                {assignment.name || playbook?.playbook.name || assignment.id}
              </span>
              <Badge variant="secondary" className="rounded-md">
                {t(`playbooks.page.scope.${assignment.scope_kind}`)}
              </Badge>
              {assignment.quick_action_enabled ? (
                <Badge variant="status-active" className="rounded-md">
                  {t("playbooks.page.fields.quickAction")}
                </Badge>
              ) : null}
              {!assignment.enabled ? (
                <Badge variant="status-neutral" className="rounded-md">
                  {t("disabled")}
                </Badge>
              ) : null}
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
      getSortValue: (assignment) =>
        playbookById.get(assignment.playbook_id)?.playbook.name ?? assignment.playbook_id,
      render: (assignment) => (
        <EntityRef
          name={playbookById.get(assignment.playbook_id)?.playbook.name}
          id={assignment.playbook_id}
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
      getSortValue: (assignment) =>
        targetSetById.get(assignment.target_set_id)?.name ?? assignment.target_set_id,
      render: (assignment) => (
        <span className="text-xs text-muted-foreground">
          {targetSetLabel(targetSetById.get(assignment.target_set_id))}
        </span>
      ),
    },
    {
      key: "scope_kind",
      label: t("playbooks.page.fields.scope"),
      filterOptions: ["global", "region", "site", "tenant", "role", "platform", "cluster"].map(
        (kind) => ({ value: kind, label: t(`playbooks.page.scope.${kind}`) }),
      ),
      getSortValue: (assignment) => assignment.scope_kind,
      render: (assignment) =>
        assignment.scope_kind === "global" ? (
          <span className="text-xs text-muted-foreground">
            {t("playbooks.page.scope.global")}
          </span>
        ) : (
          <EntityRef
            name={scopeEntities.resolveName(assignment.scope_kind, assignment.scope_id)}
            id={assignment.scope_id}
          />
        ),
    },
    {
      key: "priority",
      label: t("playbooks.page.fields.priority"),
      getSortValue: (assignment) => assignment.priority,
    },
    {
      key: "enabled",
      label: t("enabled"),
      filterOptions: boolFilterOptions({
        trueLabel: t("enabled"),
        falseLabel: t("disabled"),
      }),
      getSortValue: (assignment) => assignment.enabled,
      render: (assignment) => (
        <Badge
          variant={assignment.enabled ? "status-active" : "status-neutral"}
          className="rounded-md"
        >
          {assignment.enabled ? t("enabled") : t("disabled")}
        </Badge>
      ),
    },
    {
      key: "quick_action_enabled",
      label: t("playbooks.page.fields.quickAction"),
      filterOptions: boolFilterOptions({
        trueLabel: t("enabled"),
        falseLabel: t("disabled"),
      }),
      getSortValue: (assignment) => assignment.quick_action_enabled,
      render: (assignment) => (
        <Badge
          variant={assignment.quick_action_enabled ? "status-active" : "status-neutral"}
          className="rounded-md"
        >
          {assignment.quick_action_enabled ? t("enabled") : t("disabled")}
        </Badge>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (assignment) => (
        <AssignmentActions
          assignment={assignment}
          canUpdate={canUpdate}
          canDelete={canDelete}
          canExecute={canExecute}
          canReadTargetSet={can("READ", "PLAYBOOK_TARGET_SET")}
          runPending={runPending}
          onPreview={onPreview}
          onRun={onRun}
          onEdit={onEdit}
          onDelete={onDelete}
        />
      ),
    },
  ];

  return (
    <DataTable
      tableId="playbooks.assignments"
      className="min-h-0 flex-1"
      columns={columns}
      data={assignments}
      getRowKey={(assignment) => assignment.id}
      loading={loading}
      error={error}
      onRetry={onRetry}
      sortState={sanitizedSortState}
      onSortChange={onSortChange}
      columnFilters={sanitizedColumnFilters}
      onColumnFilterChange={onColumnFilterChange}
      pagination={pagination}
      emptyState={<ListState label={t("playbooks.page.empty.assignments")} />}
    />
  );
}

export function AssignmentActions({
  canReadTargetSet,
  assignment,
  canUpdate,
  canDelete,
  canExecute,
  runPending,
  onPreview,
  onRun,
  onEdit,
  onDelete,
}: Readonly<{
  canReadTargetSet: boolean;
  assignment: PlaybookAssignment;
  canUpdate: boolean;
  canDelete: boolean;
  canExecute: boolean;
  runPending: boolean;
  onPreview: (assignment: PlaybookAssignment) => void;
  onRun: (assignment: PlaybookAssignment) => void;
  onEdit: (assignment: PlaybookAssignment) => void;
  onDelete: (assignment: PlaybookAssignment) => void;
}>) {
  const { t } = useTranslation();
  const [menuOpen, setMenuOpen] = useState(false);
  const closeMenuThen = (action: () => void) => {
    setMenuOpen(false);
    window.setTimeout(action, 180);
  };

  return (
    <div className="flex items-center justify-end gap-2">
      {canReadTargetSet && <Button
        variant="outline"
        size="sm"
        className="gap-2"
        onClick={() => onPreview(assignment)}
      >
        <Database className="h-3.5 w-3.5" />
        {t("playbooks.page.actions.previewTargets")}
      </Button>}
      {canExecute && (
        <Button
          size="sm"
          className="gap-2"
          disabled={!assignment.enabled || !assignment.executable || runPending}
          onClick={() => onRun(assignment)}
        >
          <Play className="h-3.5 w-3.5" />
          {t("playbooks.page.actions.run")}
        </Button>
      )}
      {(canUpdate || canDelete) && (
        <RowActions label={`${t("actions")}: ${assignment.name || assignment.id}`} open={menuOpen} onOpenChange={setMenuOpen}>
            {canUpdate && (
              <DropdownMenuItem
                onSelect={(event) => {
                  event.preventDefault();
                  closeMenuThen(() => onEdit(assignment));
                }}
              >
                <Pencil className="h-3.5 w-3.5" />
                {t("edit")}
              </DropdownMenuItem>
            )}
            {canDelete && (
              <>
                {canUpdate && <DropdownMenuSeparator />}
                <DropdownMenuItem
                  variant="destructive"
                  onSelect={(event) => {
                    event.preventDefault();
                    closeMenuThen(() => onDelete(assignment));
                  }}
                >
                  <Trash2 className="h-3.5 w-3.5" />
                  {t("delete")}
                </DropdownMenuItem>
              </>
            )}
        </RowActions>
      )}
    </div>
  );
}
