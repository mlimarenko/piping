import {
  type DataTableSortKeyMap,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface ActiveConnectionsTableConfig {
  sortKeys: string[];
  sortKeyByColumn: DataTableSortKeyMap;
  filterColumns: DataTableFilterParamConfig[];
}

export const ACTIVE_CONNECTIONS_TABLE_CONFIG: ActiveConnectionsTableConfig = {
  sortKeys: ["host", "user", "protocol", "status", "started_at", "lastActivity"],
  sortKeyByColumn: {
    host: "host",
    user: "user",
    protocol: "protocol",
    status: "status",
    started_at: "started_at",
    lastActivity: "last_activity",
  },
  filterColumns: [
    { colKey: "host", param: "target_id" },
    { colKey: "user", param: "user_id" },
    { colKey: "protocol", param: "protocol" },
    { colKey: "status", param: "status" },
  ],
};

