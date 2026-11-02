import {
  serializeDataTableFilterValuesCsv,
  type DataTableFilterParamConfig,
} from "@/shared/ui-shared/tableState";

interface PlaybooksTableConfig {
  sortKeys: string[];
  filterColumns: DataTableFilterParamConfig[];
}

export const PLAYBOOK_ASSIGNMENTS_TABLE_CONFIG: PlaybooksTableConfig = {
  sortKeys: [
    "name",
    "playbook",
    "target_set",
    "scope_kind",
    "priority",
    "enabled",
    "quick_action_enabled",
  ],
  filterColumns: [
    { colKey: "playbook", param: "playbook_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "target_set", param: "target_set_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "scope_kind", param: "scope_kind", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
    {
      colKey: "quick_action_enabled",
      param: "quick_action_enabled",
      serialize: serializeDataTableFilterValuesCsv,
    },
  ],
};

export const PLAYBOOK_TARGET_SETS_TABLE_CONFIG: PlaybooksTableConfig = {
  sortKeys: ["name", "mode", "updated_at"],
  filterColumns: [
    { colKey: "mode", param: "mode", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const PLAYBOOK_SCHEDULES_TABLE_CONFIG: PlaybooksTableConfig = {
  sortKeys: ["name", "playbook", "target_set", "cron", "timezone", "concurrency", "enabled"],
  filterColumns: [
    { colKey: "playbook", param: "playbook_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "target_set", param: "target_set_id", serialize: serializeDataTableFilterValuesCsv },
    {
      colKey: "concurrency",
      param: "concurrency_policy",
      serialize: serializeDataTableFilterValuesCsv,
    },
    { colKey: "enabled", param: "enabled", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const PLAYBOOK_SERVICE_PRINCIPALS_TABLE_CONFIG: PlaybooksTableConfig = {
  sortKeys: ["name", "status", "created_at"],
  filterColumns: [
    { colKey: "status", param: "status", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const PLAYBOOK_RUN_GROUPS_TABLE_CONFIG: PlaybooksTableConfig = {
  sortKeys: ["playbook", "started_at", "status", "duration", "total_runs"],
  filterColumns: [
    { colKey: "playbook", param: "playbook_id", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "status", param: "status", serialize: serializeDataTableFilterValuesCsv },
  ],
};
