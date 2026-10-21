import {
  buildDataTableFilterParams,
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
  type DataTableSortKeyMap,
} from "@/shared/ui-shared/tableState";

interface ConnectionConfigTableConfig {
  sortKeys: string[];
  sortKeyByColumn: DataTableSortKeyMap;
  filterColumns: DataTableFilterParamConfig[];
}

export const CONNECTION_CONFIG_RECORDS_TABLE_CONFIG: ConnectionConfigTableConfig = {
  sortKeys: ["name", "description", "protocol", "baseRecord", "enabled", "owner", "createdAt", "updatedAt"],
  sortKeyByColumn: {
    name: "name",
    description: "description",
    protocol: "protocol",
    baseRecord: "base_record_id",
    enabled: "enabled",
    owner: "owner_user_id",
    createdAt: "created_at",
    updatedAt: "updated_at",
  },
  filterColumns: [
    { colKey: "protocol", param: "protocol", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "baseRecord", param: "base_record_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "owner", param: "owner_user_id", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG: ConnectionConfigTableConfig = {
  sortKeys: ["scopeType", "scope", "weight", "enabled", "createdAt"],
  sortKeyByColumn: {
    scopeType: "scope_type",
    scope: "scope_id",
    weight: "weight",
    enabled: "enabled",
    createdAt: "created_at",
  },
  filterColumns: [
    { colKey: "scopeType", param: "scope_type", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope", param: "scope_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export function buildConnectionConfigRecordFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(
      buildDataTableFilterParams(
        columnFilters,
        CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.filterColumns,
      ),
    ).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}

export function buildConnectionConfigAssignmentFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string> {
  return Object.fromEntries(
    Object.entries(
      buildDataTableFilterParams(
        columnFilters,
        CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG.filterColumns,
      ),
    ).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}
