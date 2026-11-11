import { useMemo, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Link } from "react-router";
import { Copy, Pencil, Trash2 } from "lucide-react";
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
import type { PlaybookTargetSet, PlaybookTargetSetListItem } from "@/features/playbooks/api";
import { PLAYBOOK_TAB_ROUTES } from "@/features/playbooks/playbookNavigation";
import {
  jobTemplateEditorEditHref,
  scheduleEditorEditHref,
} from "@/features/playbooks/playbookEditorRoutes";

import { formatDateTime } from "./playbookPageFormat";
import PageState from "@/shared/ui-shared/PageState";
import { PLAYBOOK_TARGET_SETS_TABLE_CONFIG } from "./tableConfig";

/** Links to the existing editors of everything that references a target set. */
function UsageLinks({ targetSet }: Readonly<{ targetSet: PlaybookTargetSetListItem }>) {
  const { t } = useTranslation();
  const links = [
    ...targetSet.usage.job_templates.map((reference) => ({
      key: `job-${reference.id}`,
      href: jobTemplateEditorEditHref(reference.id),
      label: reference.name ?? t("playbooks.page.fields.jobTemplate"),
    })),
    ...targetSet.usage.assignments.map((reference) => ({
      key: `assignment-${reference.id}`,
      href: PLAYBOOK_TAB_ROUTES.assignments,
      label: reference.name ?? t("playbooks.page.assignments.title"),
    })),
    ...targetSet.usage.schedules.map((reference) => ({
      key: `schedule-${reference.id}`,
      href: scheduleEditorEditHref(reference.id),
      label: reference.name ?? t("playbooks.page.fields.schedule"),
    })),
  ];
  if (links.length === 0) {
    return <span className="text-xs text-muted-foreground">—</span>;
  }
  const shown = links.slice(0, 3);
  return (
    <span className="flex min-w-0 flex-col gap-1 text-xs">
      {shown.map((link) => (
        <Link
          key={link.key}
          to={link.href}
          className="max-w-[18rem] truncate underline decoration-muted-foreground/40 underline-offset-2 hover:text-foreground"
          onClick={(event) => event.stopPropagation()}
        >
          {link.label}
        </Link>
      ))}
      {links.length > shown.length && (
        <span className="text-muted-foreground">+{links.length - shown.length}</span>
      )}
    </span>
  );
}

/** A known host count, or an explicit unknown state — never a fake zero. */
function TargetSetCount({ targetSet }: Readonly<{ targetSet: PlaybookTargetSetListItem }>) {
  const { t } = useTranslation();
  if (targetSet.resolved_count == null) {
    return (
      <span
        className="text-xs text-muted-foreground"
        title={t("playbooks.page.targetSets.unknownCountHint")}
      >
        {t("playbooks.page.targetSets.unknownCount")}
      </span>
    );
  }
  return <span className="text-sm tabular-nums">{targetSet.resolved_count.toLocaleString()}</span>;
}

function TargetSetComposition({ targetSet }: Readonly<{ targetSet: PlaybookTargetSetListItem }>) {
  const { t } = useTranslation();
  if (targetSet.mode === "static") {
    return (
      <span className="text-xs text-muted-foreground">
        {t("playbooks.page.selectorChips.fixedSelection")}
      </span>
    );
  }
  return <SelectorChips selector={targetSet.selector} className="block" />;
}

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
  onDuplicate,
  onSelect,
  emptyState,
  toolbar,
}: Readonly<{
  targetSets: PlaybookTargetSetListItem[];
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
  onDelete: (targetSet: PlaybookTargetSetListItem) => void;
  onDuplicate: (targetSet: PlaybookTargetSet) => void;
  onSelect: (targetSet: PlaybookTargetSet) => void;
  emptyState?: ReactNode;
  toolbar?: ReactNode;
}>) {
  const { t } = useTranslation();
  // View-but-not-edit: gate target-set CRUD affordances.
  const { can } = usePermissions();
  const canCreate = can("CREATE", "PLAYBOOK_TARGET_SET");
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
  const columns: Column<PlaybookTargetSetListItem>[] = [
    {
      key: "name",
      label: t("name"),
      sortable: true,
      getSortValue: (targetSet) => targetSet.name,
      render: (targetSet) => (
        <span className="font-medium text-sm text-foreground">{targetSet.name}</span>
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
      key: "count",
      label: t("playbooks.page.fields.count"),
      render: (targetSet) => <TargetSetCount targetSet={targetSet} />,
    },
    {
      key: "composition",
      label: t("playbooks.page.fields.composition"),
      render: (targetSet) => <TargetSetComposition targetSet={targetSet} />,
    },
    {
      key: "used_by",
      label: t("playbooks.page.fields.usedBy"),
      render: (targetSet) => <UsageLinks targetSet={targetSet} />,
    },
    {
      key: "updated_at",
      label: t("playbooks.page.fields.updated"),
      sortable: true,
      getSortValue: (targetSet) => targetSet.updated_at,
      render: (targetSet) => (
        <span className="text-xs text-muted-foreground">
          {formatDateTime(targetSet.updated_at)}
        </span>
      ),
    },
    {
      key: "owner",
      label: t("playbooks.page.fields.owner"),
      render: (targetSet) => (
        <span className="text-xs text-muted-foreground">
          {targetSet.updated_by_username ?? targetSet.created_by_username ?? "—"}
        </span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (targetSet) =>
        canCreate || canUpdate || canDelete ? (
          <TargetSetActions
            targetSet={targetSet}
            canCreate={canCreate}
            canUpdate={canUpdate}
            canDelete={canDelete}
            onEdit={onEdit}
            onDelete={onDelete}
            onDuplicate={onDuplicate}
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
      toolbar={toolbar}
      emptyState={
        emptyState ?? <PageState kind="empty" title={t("playbooks.page.empty.targetSets")} />
      }
    />
  );
}

export function TargetSetActions({
  targetSet,
  canCreate,
  canUpdate,
  canDelete,
  onEdit,
  onDelete,
  onDuplicate,
}: Readonly<{
  targetSet: PlaybookTargetSetListItem;
  canCreate: boolean;
  canUpdate: boolean;
  canDelete: boolean;
  onEdit: (targetSet: PlaybookTargetSet) => void;
  onDelete: (targetSet: PlaybookTargetSetListItem) => void;
  onDuplicate: (targetSet: PlaybookTargetSet) => void;
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
        {canCreate && (
          <DropdownMenuItem
            onSelect={(event) => {
              event.preventDefault();
              closeMenuThen(() => onDuplicate(targetSet));
            }}
          >
            <Copy className="h-3.5 w-3.5" />
            {t("playbooks.page.actions.duplicateTargetSet")}
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
