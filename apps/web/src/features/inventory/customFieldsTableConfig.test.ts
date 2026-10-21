import { describe, expect, it } from "vitest";

import {
  CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG,
  CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG,
} from "./customFieldsTableConfig";

describe("custom fields table config", () => {
  it("keeps custom field definition params declarative", () => {
    expect(CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG.sortKeys).toEqual([
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
    ]);
    expect(
      CUSTOM_FIELD_DEFINITIONS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([
      { colKey: "field_type", param: "field_type" },
      { colKey: "group_name", param: "group_name" },
      { colKey: "ui_visible", param: "ui_visible" },
      { colKey: "ui_editable", param: "ui_editable" },
      { colKey: "filter_logic", param: "filter_logic" },
      { colKey: "choice_set", param: "choice_set_id" },
      { colKey: "related_object_type", param: "related_object_type_id" },
      { colKey: "scope", param: "object_type_id" },
    ]);
  });

  it("keeps choice-set sort keys declarative", () => {
    expect(CUSTOM_FIELD_CHOICE_SETS_TABLE_CONFIG.sortKeys).toEqual(["name", "choices"]);
  });
});
