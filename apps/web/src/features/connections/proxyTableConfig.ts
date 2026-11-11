import {
  serializeDataTableFilterValuesCsv,
  type DataTableSortKeyMap,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface ProxyTableConfig {
  sortKeys: string[];
  sortKeyByColumn: DataTableSortKeyMap;
  filterColumns: DataTableFilterParamConfig[];
}

export const PROXY_NODES_TABLE_CONFIG: ProxyTableConfig = {
  sortKeys: ["name", "slug", "baseUrl", "enabled"],
  sortKeyByColumn: {
    name: "name",
    slug: "slug",
    baseUrl: "base_url",
    enabled: "enabled",
  },
  filterColumns: [
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const PROXY_ASSIGNMENTS_TABLE_CONFIG: ProxyTableConfig = {
  sortKeys: ["proxy", "scope_type", "weight", "enabled"],
  sortKeyByColumn: {
    proxy: "proxy_node_id",
    scope_type: "scope_type",
    weight: "weight",
    enabled: "enabled",
  },
  filterColumns: [
    { colKey: "proxy", param: "proxy_node_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope_type", param: "scope_type", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
  ],
};
