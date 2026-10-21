import { useRef, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import DataTable, { type Column, type DataTableFilterOption } from "@/shared/ui-shared/DataTable";
import { PageActions } from "@/shared/ui-shared/PageChrome";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem, DropdownMenuSeparator } from "@/shared/ui-primitives/dropdown-menu";
import { Button } from "@/shared/ui-primitives/button";
import SearchInput from "@/shared/ui-shared/SearchInput";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/shared/ui-primitives/alert-dialog";
import { Pencil, Plus, Trash2 } from "lucide-react";
import { useAccessListTable } from "@/features/access-control/useAccessListTable";
import type { AccessTableControls } from "@/features/access-control/useAccessTableControls";
import type { AccessFilterContext, AccessTableConfig } from "@/features/access-control/tableConfig";
import type { AccessListQuery } from "@/features/access-control/api";
import type { Paginated } from "@/shared/api/types";

/** One extra command in a row's action menu (beyond the shared edit/delete). */
export interface AccessRowAction<T> {
  key: string;
  label: ReactNode;
  icon: ReactNode;
  /** Called with the row and the row-action trigger element (for focus restore). */
  onSelect: (row: T, trigger: HTMLButtonElement | null) => void;
}

export interface AccessEntityListProps<T extends { id: string }> {
  /** Stable DataTable id (e.g. `access.groups`). */
  tableId: string;
  /** Sort/filter consumer contract (see `tableConfig.ts`). */
  config: AccessTableConfig;
  /** Base TanStack Query key under which the paginated key is derived. */
  queryKeyPrefix: readonly unknown[];
  /** The paginated list endpoint. */
  listFn: (query: AccessListQuery) => Promise<Paginated<T>>;
  getRowKey: (row: T) => string;
  /** Human label used for the row's `Actions: <label>` aria. */
  rowLabel: (row: T) => string;
  searchPlaceholder: string;
  emptyState: ReactNode;
  /** Data columns only; the actions column is appended by this renderer.
   *  `columnMeta` maps a column key to its sortable/filterable attributes. */
  columns: (columnMeta: AccessTableControls["columnMeta"]) => Column<T>[];
  fkOptions?: Record<string, DataTableFilterOption[]>;
  filterContext?: AccessFilterContext;
  canCreate: boolean;
  canUpdate: boolean;
  canDelete: boolean;
  /** Header create button label; omit (with no `prefix`) for read-only. */
  createLabel?: string;
  onCreate?: () => void;
  /** Optional edit command; omit to hide the edit menu item (grants). */
  onEdit?: (row: T) => void;
  /** Extra row commands (e.g. the users password reset). */
  extraActions?: (row: T) => AccessRowAction<T>[];
  /** EntityEditorHost rendered by the caller (create/edit dialog). */
  editor?: ReactNode;
  /** Optional content above the table (grants inline add card). */
  prefix?: ReactNode;
  deleteTitle: string;
  deleteDescription: (row: T | null) => ReactNode;
  deleteActionLabel: string;
  onDelete: (id: string) => Promise<void>;
}

/**
 * Config-driven list composition shared by the four directory tables
 * (users / groups / roles / grants): paginated DataTable + search toolbar +
 * capability-gated RowActions (edit / delete / extras) + delete confirmation.
 *
 * The caller keeps only what is genuinely per-resource — its columns, the editor
 * form (handed in as `editor`), and its create/delete callbacks. The duplicated
 * list scaffold (page/page-size/search/query), the action-menu focus bookkeeping,
 * and the delete confirm that used to be copy-pasted into each table live here.
 */
export function AccessEntityList<T extends { id: string }>({
  tableId,
  config,
  queryKeyPrefix,
  listFn,
  getRowKey,
  rowLabel,
  searchPlaceholder,
  emptyState,
  columns,
  fkOptions,
  filterContext,
  canCreate,
  canUpdate,
  canDelete,
  createLabel,
  onCreate,
  onEdit,
  extraActions,
  editor,
  prefix,
  deleteTitle,
  deleteDescription,
  deleteActionLabel,
  onDelete,
}: AccessEntityListProps<T>) {
  const { t } = useTranslation();
  const table = useAccessListTable<T>({ config, queryKeyPrefix, listFn, fkOptions, filterContext });

  const actionTriggers = useRef(new Map<string, HTMLButtonElement>());
  const deleteInvoker = useRef<HTMLButtonElement | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<T | null>(null);

  const focusRowAction = (row: T) => actionTriggers.current.get(getRowKey(row))?.focus();
  const openDelete = (row: T) => {
    deleteInvoker.current = actionTriggers.current.get(getRowKey(row)) ?? null;
    deleteInvoker.current?.focus();
    setDeleteTarget(row);
  };
  const handleDelete = async () => {
    if (!deleteTarget) return;
    await onDelete(deleteTarget.id);
    setDeleteTarget(null);
  };

  const actionsColumn: Column<T> = {
    key: "actions",
    label: "",
    excludeFromSettings: true,
    render: (row) => {
      const extras = extraActions?.(row) ?? [];
      const hasEdit = Boolean(onEdit) && canUpdate;
      const hasDelete = canDelete;
      if (!hasEdit && !hasDelete && extras.length === 0) return null;
      return (
        <RowActions
          label={`${t("actions")}: ${rowLabel(row)}`}
          ref={(node) => {
            if (node) actionTriggers.current.set(getRowKey(row), node);
            else actionTriggers.current.delete(getRowKey(row));
          }}
        >
          {extras.map((action) => (
            <DropdownMenuItem
              key={action.key}
              onSelect={() => {
                const trigger = actionTriggers.current.get(getRowKey(row)) ?? null;
                trigger?.focus();
                action.onSelect(row, trigger);
              }}
            >
              {action.icon}{action.label}
            </DropdownMenuItem>
          ))}
          {hasEdit && (
            <DropdownMenuItem
              onSelect={() => {
                focusRowAction(row);
                onEdit?.(row);
              }}
            >
              <Pencil />{t("edit")}
            </DropdownMenuItem>
          )}
          {(hasEdit || extras.length > 0) && hasDelete && <DropdownMenuSeparator />}
          {hasDelete && (
            <DropdownMenuItem variant="destructive" onSelect={() => openDelete(row)}>
              <Trash2 />{deleteActionLabel}
            </DropdownMenuItem>
          )}
        </RowActions>
      );
    },
  };

  return (
    <div className="flex min-w-0 flex-col gap-4">
      {createLabel && canCreate && (
        <PageActions>
          <Button size="sm" onClick={onCreate}>
            <Plus className="size-4" /> {createLabel}
          </Button>
        </PageActions>
      )}
      {editor}
      {prefix}
      <DataTable
        tableId={tableId}
        className="min-h-0"
        columns={[...columns(table.controls.columnMeta), actionsColumn]}
        data={table.items}
        getRowKey={getRowKey}
        loading={table.query.isPending}
        error={table.query.error}
        onRetry={() => table.query.refetch()}
        emptyState={emptyState}
        pagination={{
          page: table.page,
          pageSize: table.pageSize,
          total: table.total,
          onPageChange: table.handlePageChange,
          onPageSizeChange: table.handlePageSizeChange,
        }}
        sortState={table.controls.sortState}
        onSortChange={table.controls.onSortChange}
        columnFilters={table.controls.columnFilters}
        onColumnFilterChange={table.controls.onColumnFilterChange}
        toolbar={
          <SearchInput
            containerClassName="flex-1 max-w-xs"
            value={table.search}
            onValueChange={table.handleSearchChange}
            placeholder={searchPlaceholder}
          />
        }
      />

      <AlertDialog open={!!deleteTarget} onOpenChange={(open) => !open && setDeleteTarget(null)}>
        <AlertDialogContent
          className="border border-border bg-card"
          onCloseAutoFocus={(event) => {
            if (deleteInvoker.current?.isConnected) {
              event.preventDefault();
              deleteInvoker.current.focus();
            }
          }}
        >
          <AlertDialogHeader>
            <AlertDialogTitle>{deleteTitle}</AlertDialogTitle>
            <AlertDialogDescription>{deleteDescription(deleteTarget)}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction
              onClick={handleDelete}
              className="bg-destructive text-destructive-foreground hover:bg-destructive/90"
            >
              {deleteActionLabel}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
