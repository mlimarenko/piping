import {
  buildDataTableFilterParams,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface SyncMappingsTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
}

export const SYNC_MAPPINGS_TABLE_CONFIG: SyncMappingsTableConfig = {
  sortKeys: ["name", "versions", "direction", "provider_id"],
  filterColumns: [
    { colKey: "direction", param: "direction" },
  ],
};

export function buildSyncMappingsFilterParams(
  columnFilters: Record<string, string[]>,
): { direction?: string[] } {
  const params = buildDataTableFilterParams(
    columnFilters,
    SYNC_MAPPINGS_TABLE_CONFIG.filterColumns,
  );
  return {
    direction: Array.isArray(params.direction) ? params.direction : undefined,
  };
}
