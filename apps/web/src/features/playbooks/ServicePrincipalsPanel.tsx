import { useMemo } from "react";
import { useTranslation } from "react-i18next";
import { KeyRound, ShieldOff } from "lucide-react";

import DataTable, { type Column, type DataTableSortState } from "@/shared/ui-shared/DataTable";
import type { TablePaginationProps } from "@/shared/ui-shared/Pagination";
import { Badge } from "@/shared/ui-primitives/badge";
import RowActions from "@/shared/ui-shared/RowActions";
import { DropdownMenuItem } from "@/shared/ui-primitives/dropdown-menu";
import EntityRef from "@/shared/ui-shared/EntityRef";
import {
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
} from "@/shared/ui-shared/tableState";
import { usePermissions } from "@/shared/auth/permissionsContext";
import type { PlaybookServicePrincipal } from "@/features/playbooks/api";

import { formatDateTime } from "./playbookPageFormat";
import { ListState } from "./PlaybookPanelUi";
import { PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG } from "./tableConfig";

/** A system-seeded principal carries `metadata.system === true` — a structural
 *  flag, not a name/slug match, so this stays domain-neutral. Its `description`
 *  is an English seed string and it is non-revocable, so the table localizes the
 *  description, tags it, and hides the (would-fail) revoke action rather than
 *  surfacing raw backend prose or a dead-end button. */
function isSystemServicePrincipal(principal: PlaybookServicePrincipal): boolean {
  const metadata = principal.metadata;
  return (
    typeof metadata === "object" &&
    metadata !== null &&
    (metadata as Record<string, unknown>).system === true
  );
}

export function ServicePrincipalsPanel({
  servicePrincipals,
  loading,
  error,
  onRetry,
  sortState,
  onSortChange,
  columnFilters,
  onColumnFilterChange,
  pagination,
  onRevoke,
}: Readonly<{
  servicePrincipals: PlaybookServicePrincipal[];
  loading: boolean;
  error: unknown;
  onRetry: () => void;
  sortState: DataTableSortState | null;
  onSortChange: (next: DataTableSortState | null) => void;
  columnFilters: Record<string, string[]>;
  onColumnFilterChange: (key: string, values: string[]) => void;
  pagination?: TablePaginationProps;
  onRevoke: (principal: PlaybookServicePrincipal) => void;
}>) {
  const { t } = useTranslation();
  // View-but-not-edit: gate principal create + revoke (revoke is a DELETE).
  const { can } = usePermissions();
  const canRevoke = can("DELETE", "PLAYBOOK_SERVICE_PRINCIPAL");
  const sanitizedSortState = useMemo(
    () =>
      sanitizeDataTableSortState(
        sortState,
        PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG.sortKeys,
      ),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const columns: Column<PlaybookServicePrincipal>[] = [
    {
      key: "name",
      label: t("playbooks.page.fields.servicePrincipalName"),
      getSortValue: (principal) => principal.name,
      render: (principal) => {
        const system = isSystemServicePrincipal(principal);
        return (
          <div className="min-w-0">
            <div className="flex flex-wrap items-center gap-2">
              <EntityRef
                name={principal.name}
                id={principal.id}
                icon={KeyRound}
                className="font-medium text-foreground"
              />
              <Badge variant="secondary" className="rounded-md font-mono">
                {principal.slug}
              </Badge>
              {system && (
                <Badge variant="outline" className="rounded-md">
                  {t("playbooks.page.servicePrincipals.systemBadge")}
                </Badge>
              )}
            </div>
            <p className="mt-0.5 line-clamp-1 max-w-[40ch] text-xs text-muted-foreground">
              {system
                ? t("playbooks.page.servicePrincipals.systemDescription")
                : principal.description || t("playbooks.page.servicePrincipals.noDescription")}
            </p>
          </div>
        );
      },
    },
    {
      key: "status",
      label: t("status"),
      filterOptions: ["active", "disabled", "revoked"].map((status) => ({
        value: status,
        label: t(`playbooks.page.servicePrincipals.status.${status}`, status),
      })),
      getSortValue: (principal) => principal.status,
      render: (principal) => (
        <Badge
          variant={principal.status === "active" ? "status-active" : "status-neutral"}
          className="rounded-md"
        >
          {t(`playbooks.page.servicePrincipals.status.${principal.status}`, principal.status)}
        </Badge>
      ),
    },
    {
      key: "created_at",
      label: t("playbooks.page.fields.created"),
      getSortValue: (principal) => principal.created_at,
      render: (principal) => (
        <span className="text-xs text-muted-foreground">{formatDateTime(principal.created_at)}</span>
      ),
    },
    {
      key: "actions",
      label: "",
      excludeFromSettings: true,
      className: "text-right",
      render: (principal) =>
        principal.status === "active" && canRevoke && !isSystemServicePrincipal(principal) ? (
          <RowActions label={`${t("actions")}: ${principal.name}`}>
            <DropdownMenuItem variant="destructive" onSelect={() => window.setTimeout(() => onRevoke(principal), 180)}>
              <ShieldOff className="size-4" />
              {t("playbooks.page.actions.revoke")}
            </DropdownMenuItem>
          </RowActions>
        ) : null,
    },
  ];

  return (
    <DataTable
      tableId="playbooks.servicePrincipals"
      className="min-h-0 flex-1"
      columns={columns}
      data={servicePrincipals}
      getRowKey={(principal) => principal.id}
      loading={loading}
      error={error}
      onRetry={onRetry}
      sortState={sanitizedSortState}
      onSortChange={onSortChange}
      columnFilters={sanitizedColumnFilters}
      onColumnFilterChange={onColumnFilterChange}
      pagination={pagination}
      emptyState={<ListState label={t("playbooks.page.empty.servicePrincipals")} />}
    />
  );
}
