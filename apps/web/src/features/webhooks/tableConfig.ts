import {
  buildDataTableFilterParams,
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface WebhooksTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
}

export const WEBHOOKS_TABLE_CONFIG: WebhooksTableConfig = {
  sortKeys: [
    "name",
    "target_url",
    "secret_set",
    "event_types",
    "enabled",
    "last_fired_at",
    "last_status",
  ],
  filterColumns: [
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "secret_set", param: "secret_set", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "event_types", param: "event_types", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "last_status", param: "last_status", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export function buildWebhookFilterParams(columnFilters: Record<string, string[]>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(
      buildDataTableFilterParams(columnFilters, WEBHOOKS_TABLE_CONFIG.filterColumns),
    ).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}
