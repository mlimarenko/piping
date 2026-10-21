import {
  buildDataTableFilterParams,
  type DataTableSortKeyMap,
  type DataTableFilterParamConfig,
  type DataTableFilterParamValue,
} from "@/shared/ui-shared/tableState";

interface UserSettingsTableConfig {
  sortKeys: string[];
  sortKeyByColumn: DataTableSortKeyMap;
  filterColumns: DataTableFilterParamConfig[];
}

export const USER_CREDENTIALS_TABLE_CONFIG: UserSettingsTableConfig = {
  sortKeys: ["protocol", "name", "isDefault"],
  sortKeyByColumn: {
    protocol: "protocol",
    name: "name",
    isDefault: "is_default",
  },
  filterColumns: [
    { colKey: "protocol", param: "protocol" },
    { colKey: "isDefault", param: "is_default" },
  ],
};

export const USER_TOKENS_TABLE_CONFIG: UserSettingsTableConfig = {
  sortKeys: ["name", "scopes", "status", "createdAt", "expiresAt"],
  sortKeyByColumn: {
    name: "name",
    scopes: "scopes",
    status: "status",
    createdAt: "created_at",
    expiresAt: "expires_at",
  },
  filterColumns: [
    { colKey: "scopes", param: "scope" },
    { colKey: "status", param: "status" },
  ],
};

function asStringArrayParams(
  params: Record<string, DataTableFilterParamValue>,
): Record<string, string[]> {
  return Object.fromEntries(
    Object.entries(params).filter(
      (entry): entry is [string, string[]] => Array.isArray(entry[1]),
    ),
  );
}

export function buildUserCredentialFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string[]> {
  return asStringArrayParams(
    buildDataTableFilterParams(
      columnFilters,
      USER_CREDENTIALS_TABLE_CONFIG.filterColumns,
    ),
  );
}

export function buildUserTokenFilterParams(
  columnFilters: Record<string, string[]>,
): Record<string, string[]> {
  return asStringArrayParams(
    buildDataTableFilterParams(columnFilters, USER_TOKENS_TABLE_CONFIG.filterColumns),
  );
}
