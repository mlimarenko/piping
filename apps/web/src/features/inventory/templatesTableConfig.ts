import {
  serializeDataTableFilterValuesCsv,
  type DataTableQueryConfig,
} from "@/shared/ui-shared/tableState";

export const INVENTORY_TEMPLATES_TABLE_CONFIG: DataTableQueryConfig = {
  sortKeys: ["name", "slug", "shape", "status", "updatedAt"],
  filterColumns: [
    { colKey: "status", param: "status", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG: DataTableQueryConfig = {
  sortKeys: ["status", "template", "startedAt", "duration", "steps"],
  filterColumns: [
    { colKey: "status", param: "status", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "template", param: "template_id", serialize: serializeDataTableFilterValuesCsv },
  ],
};
