import { useMemo, useState } from "react";
import { Link, useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { useQuery } from "@tanstack/react-query";
import {
  Plus, Network, GitBranch,
} from "lucide-react";
import { usePermissions } from "@/shared/auth/permissionsContext";
import { listUsers, usersAllQueryKey } from "@/features/access-control/api";
import PageChrome from "@/shared/ui-shared/PageChrome";
import DataTable, {
  type Column,
  type DataTableFilterOption,
  type DataTableSortState,
} from "@/shared/ui-shared/DataTable";
import { boolFilterOptions } from "@/shared/ui-shared/dataTableLocal";
import SearchInput from "@/shared/ui-shared/SearchInput";
import StatusChip from "@/shared/ui-shared/StatusChip";
import EmptyState from "@/shared/ui-shared/EmptyState";
import {
  mapDataTableSortKey,
  sanitizeDataTableColumnFilters,
  sanitizeDataTableSortState,
  updateDataTableColumnFilters,
} from "@/shared/ui-shared/tableState";
import { Button } from "@/shared/ui-primitives/button";
import {
  listRecords,
  type ConnConfRecord,
} from "./api";
import {
  buildConnectionConfigRecordFilterParams,
  CONNECTION_CONFIG_RECORDS_TABLE_CONFIG,
} from "./tableConfig";
import { CONNECTION_CONFIG_ROUTE, connectionConfigRecordHref } from "./recordRoutes";

function formatDateTime(value?: string | null): string {
  return value ? new Date(value).toLocaleString() : "—";
}


const ConnectionConfig = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { can } = usePermissions();
  const canCreate = can("CREATE", "CONFIG_CONTEXT");
  const [searchQuery, setSearchQuery] = useState("");
  const [sortState, setSortState] = useState<DataTableSortState | null>(null);
  const [columnFilters, setColumnFilters] = useState<Record<string, string[]>>({});

  const sanitizedSortState = useMemo(
    () =>
      sanitizeDataTableSortState(
        sortState,
        CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.sortKeys,
      ),
    [sortState],
  );
  const sanitizedColumnFilters = useMemo(
    () =>
      sanitizeDataTableColumnFilters(
        columnFilters,
        CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.filterColumns.map((column) => column.colKey),
      ),
    [columnFilters],
  );
  const recordFilterParams = useMemo(
    () => buildConnectionConfigRecordFilterParams(sanitizedColumnFilters),
    [sanitizedColumnFilters],
  );

  const recordsQuery = useQuery({
    queryKey: ["connection-config", "records", sanitizedSortState, recordFilterParams],
    queryFn: () =>
      listRecords({
        sort: mapDataTableSortKey(
          sanitizedSortState,
          CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.sortKeyByColumn,
        ),
        order: sanitizedSortState?.dir,
        protocol: recordFilterParams.protocol,
        base_record_id: recordFilterParams.base_record_id,
        owner_user_id: recordFilterParams.owner_user_id,
        enabled: recordFilterParams.enabled,
      }),
  });
  const allRecordsQuery = useQuery({
    queryKey: ["connection-config", "records", "options"],
    queryFn: () => listRecords(),
    staleTime: 60_000,
  });
  const usersQuery = useQuery({
    queryKey: usersAllQueryKey(),
    queryFn: () => listUsers({ limit: 500 }),
    staleTime: 60_000,
  });
  const records = useMemo(() => recordsQuery.data ?? [], [recordsQuery.data]);
  const allRecords = useMemo(
    () => allRecordsQuery.data ?? records,
    [allRecordsQuery.data, records],
  );
  const users = useMemo(() => usersQuery.data?.items ?? [], [usersQuery.data]);

  const filtered = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    if (!q) return records;
    return records.filter(
      (record) =>
        record.name.toLowerCase().includes(q) ||
        record.protocol.toLowerCase().includes(q) ||
        record.description.toLowerCase().includes(q),
    );
  }, [records, searchQuery]);

  const recordOptions = useMemo<DataTableFilterOption[]>(
    () =>
      allRecords.map((record) => ({
        value: record.id,
        label: record.name,
      })),
    [allRecords],
  );
  const ownerOptions = useMemo<DataTableFilterOption[]>(
    () =>
      users.map((user) => ({
        value: user.id,
        label: user.username,
      })),
    [users],
  );
  const filterOptionsByKey = useMemo<Record<string, DataTableFilterOption[]>>(
    () => ({
      protocol: ["ssh", "rdp", "vnc", "app", "cctv"].map((value) => ({
        value,
        label: value.toUpperCase(),
      })),
      enabled: boolFilterOptions({
        trueLabel: t("enabled"),
        falseLabel: t("disabled"),
      }),
      baseRecord: recordOptions,
      owner: ownerOptions,
    }),
    [ownerOptions, recordOptions, t],
  );
  const columns = useMemo<Column<ConnConfRecord>[]>(
    () => [
      {
        key: "name",
        label: t("connectionConfig.records.columns.name"),
        width: "32%",
        sortable: true,
        render: (record) => (
          <div className="min-w-0">
            <div className="flex items-center gap-1.5">
              <Link to={connectionConfigRecordHref(record.id)} className="truncate font-medium text-primary hover:underline">{record.name}</Link>
              {record.base_record_id ? (
                <GitBranch className="h-3 w-3 shrink-0 text-muted-foreground/60" />
              ) : null}
            </div>
            {record.description ? (
              <span className="block truncate text-xs text-muted-foreground">
                {record.description}
              </span>
            ) : null}
          </div>
        ),
      },
      {
        key: "description",
        label: t("connectionConfig.records.columns.description"),
        width: "24%",
        sortable: true,
        render: (record) => (
          <span className="block max-w-[16rem] truncate text-muted-foreground">
            {record.description || "—"}
          </span>
        ),
      },
      {
        key: "protocol",
        label: t("connectionConfig.records.columns.protocol"),
        width: "7rem",
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.protocol,
        render: (record) => (
          <span className="surface-chip inline-flex rounded px-2 py-0.5 text-xs font-semibold uppercase text-foreground">
            {record.protocol}
          </span>
        ),
      },
      {
        key: "baseRecord",
        label: t("connectionConfig.records.columns.baseRecord"),
        width: "12rem",
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.baseRecord,
        render: (record) =>
          record.base_record_id
            ? allRecords.find((candidate) => candidate.id === record.base_record_id)?.name ?? record.base_record_id
            : t("connectionConfig.fields.baseRecordNone"),
      },
      {
        key: "enabled",
        label: t("connectionConfig.records.columns.enabled"),
        width: "9rem",
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.enabled,
        render: (record) => (
          <StatusChip
            status={record.enabled ? "positive" : "neutral"}
            label={record.enabled ? t("enabled") : t("disabled")}
          />
        ),
      },
      {
        key: "owner",
        label: t("connectionConfig.records.columns.owner"),
        width: "12rem",
        sortable: true,
        filterable: true,
        filterOptions: filterOptionsByKey.owner,
        render: (record) =>
          record.owner_user_id
            ? users.find((user) => user.id === record.owner_user_id)?.username ?? record.owner_user_id
            : "—",
      },
      {
        key: "createdAt",
        label: t("connectionConfig.records.columns.createdAt"),
        width: "12rem",
        sortable: true,
        render: (record) => formatDateTime(record.created_at),
      },
      {
        key: "updatedAt",
        label: t("connectionConfig.records.columns.updatedAt"),
        width: "12rem",
        sortable: true,
        render: (record) => formatDateTime(record.updated_at),
      },
    ],
    [allRecords, filterOptionsByKey, t, users],
  );

  const listCount = filtered.length;
  return (
    <div className="flex min-w-0 flex-col gap-4">
      <PageChrome
        title={t("connectionConfig.title")}
        description={t("connectionConfig.description")}
        icon={Network}
        badge={recordsQuery.isSuccess ? String(records.length) : undefined}
        actions={
          canCreate ? (
            <Button asChild><Link to={`${CONNECTION_CONFIG_ROUTE}/new`}>
              <Plus className="size-4" /> {t("connectionConfig.newRecord")}
            </Link>
            </Button>
          ) : undefined
        }
      />

       <div className="min-w-0">
        <DataTable
          tableId="connection-config.records"
          data={filtered}
          columns={columns}
          getRowKey={(record) => record.id}
           onRowClick={(record) => { void navigate(connectionConfigRecordHref(record.id)); }}
          sortState={sanitizedSortState}
          onSortChange={setSortState}
          columnFilters={sanitizedColumnFilters}
          onColumnFilterChange={(key, values) => {
            setColumnFilters((prev) => updateDataTableColumnFilters(prev, key, values));
          }}
          loading={recordsQuery.isPending || allRecordsQuery.isPending || usersQuery.isPending}
          error={recordsQuery.error ?? allRecordsQuery.error ?? usersQuery.error}
          onRetry={() => {
            void recordsQuery.refetch();
            void allRecordsQuery.refetch();
            void usersQuery.refetch();
          }}
          toolbar={
            <div className="flex min-w-0 flex-1 flex-wrap items-center gap-2">
              <SearchInput
                value={searchQuery}
                onValueChange={setSearchQuery}
                placeholder={t("connectionConfig.searchRecords")}
              />
              <span className="shrink-0 surface-chip rounded-full px-2 py-0.5 text-xs text-muted-foreground tabular-nums">
                {t("connectionConfig.records.count", {
                  count: listCount,
                  defaultValue: "{{count}} records",
                })}
              </span>
            </div>
          }
          emptyState={
            <EmptyState
              icon={Network}
              title={
                searchQuery
                  ? t("connectionConfig.noResults")
                  : t("connectionConfig.noRecords")
              }
              actionLabel={searchQuery || !canCreate ? undefined : t("connectionConfig.newRecord")}
               onAction={searchQuery || !canCreate ? undefined : () => { void navigate(`${CONNECTION_CONFIG_ROUTE}/new`); }}
              className="py-12"
            />
          }
        />

      </div>
    </div>
  );
};

export default ConnectionConfig;
