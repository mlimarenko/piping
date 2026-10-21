import {
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface ObservabilityTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
}

export const EXPORTER_ASSIGNMENTS_TABLE_CONFIG: ObservabilityTableConfig = {
  sortKeys: ["exporter", "scope_type", "scope_id", "job", "weight", "enabled"],
  filterColumns: [
    { colKey: "exporter", param: "exporter_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope_type", param: "scope_type", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope_id", param: "scope_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
  ],
};
