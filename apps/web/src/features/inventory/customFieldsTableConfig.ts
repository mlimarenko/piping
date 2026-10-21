import {
  serializeDataTableFilterValuesCsv,
  type DataTableQueryConfig,
} from "@/shared/ui-shared/tableState";

export const CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG: DataTableQueryConfig = {
  sortKeys: [
    "label",
    "name",
    "field_type",
    "group_name",
    "weight",
    "ui_visible",
    "ui_editable",
    "filter_logic",
    "choice_set",
    "related_object_type",
    "scope",
    "flags",
  ],
  filterColumns: [
    { colKey: "field_type", param: "field_type", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "group_name", param: "group_name", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "ui_visible", param: "ui_visible", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "ui_editable", param: "ui_editable", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "filter_logic", param: "filter_logic", serialize: serializeDataTableFilterValuesCsv },
    { colKey: "choice_set", param: "choice_set_id", serialize: serializeDataTableFilterValuesCsv },
    {
      colKey: "related_object_type",
      param: "related_object_type_id",
      serialize: serializeDataTableFilterValuesCsv,
    },
    { colKey: "scope", param: "object_type_id", serialize: serializeDataTableFilterValuesCsv },
  ],
};

export const CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG: DataTableQueryConfig = {
  sortKeys: ["name", "choices"],
  filterColumns: [],
};
