import {
  buildDataTableFilterParams,
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface ConfigContextBindingsTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
}

export const CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG: ConfigContextBindingsTableConfig = {
  sortKeys: ["scope_type", "scope_id"],
  filterColumns: [
    {
      colKey: "scope_type",
      param: "scope_type",
      serialize: serializeDataTableFilterValuesCsv,
    },
  ],
};

export function buildConfigContextBindingsFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(
      buildDataTableFilterParams(
        columnFilters,
        CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG.filterColumns,
      ),
    ).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}
