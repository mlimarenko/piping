import {
  buildDataTableFilterParams,
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
  type DataTableSortKeyMap,
} from "@/shared/ui-shared/tableState";

interface SyncProvidersTableConfig {
  sortKeys: string[];
  sortKeyByColumn: DataTableSortKeyMap;
  filterColumns: DataTableFilterParamConfig[];
}

export const SYNC_PROVIDERS_TABLE_CONFIG: SyncProvidersTableConfig = {
  sortKeys: ["name", "providerType", "enabled"],
  sortKeyByColumn: {
    name: "name",
    providerType: "provider_type",
    enabled: "is_enabled",
  },
  filterColumns: [
    {
      colKey: "providerType",
      param: "provider_type",
      serialize: serializeDataTableFilterValuesCsv,
    },
    {
      colKey: "enabled",
      param: "is_enabled",
      serialize: serializeDataTableFilterValuesCsv,
    },
  ],
};

export function buildSyncProvidersFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(
      buildDataTableFilterParams(
        columnFilters,
        SYNC_PROVIDERS_TABLE_CONFIG.filterColumns,
      ),
    ).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}
