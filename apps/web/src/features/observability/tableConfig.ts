import {
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface ObservabilityTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
}

export const PROFILE_ASSIGNMENTS_TABLE_CONFIG: ObservabilityTableConfig = {
  sortKeys: ["profile", "scope_type", "scope_id", "target", "enabled"],
  filterColumns: [
    { colKey: "profile", param: "profile_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope_type", param: "scope_type", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope_id", param: "scope_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
  ],
};
