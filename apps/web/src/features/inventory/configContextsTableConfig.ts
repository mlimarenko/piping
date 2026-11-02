import {
  buildDataTableFilterParams,
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
  type DataTableSortKeyMap,
} from "@/shared/ui-shared/tableState";

interface ConfigContextsTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
  sortKeyByColumn: DataTableSortKeyMap;
}

export const CONFIG_CONTEXTS_TABLE_CONFIG: ConfigContextsTableConfig = {
  sortKeys: ["name", "weight", "status"],
  filterColumns: [
    { colKey: "status", param: "is_active", serialize: serializeDataTableFilterValuesCsv },
  ],
  sortKeyByColumn: {
    status: "status",
  },
};

export function buildConfigContextsFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(
      buildDataTableFilterParams(
        columnFilters,
        CONFIG_CONTEXTS_TABLE_CONFIG.filterColumns,
      ),
    ).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}

export function configContextsSortParam(key: string | undefined): string | undefined {
  if (!key) return undefined;
  return CONFIG_CONTEXTS_TABLE_CONFIG.sortKeyByColumn[key] ?? key;
}
