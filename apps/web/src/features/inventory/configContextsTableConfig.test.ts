import { describe, expect, it } from "vitest";

import {
  buildConfigContextsFilterParams,
  CONFIG_CONTEXTS_TABLE_CONFIG,
  configContextsSortParam,
} from "./configContextsTableConfig";

describe("config contexts table config", () => {
  it("keeps config-context sort and filter params declarative", () => {
    expect(CONFIG_CONTEXTS_TABLE_CONFIG.sortKeys).toEqual(["name", "weight", "status"]);
    expect(
      CONFIG_CONTEXTS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([{ colKey: "status", param: "is_active" }]);
    expect(configContextsSortParam("status")).toBe("status");
    expect(buildConfigContextsFilterParams({ status: ["true"] })).toEqual({ is_active: "true" });
    expect(buildConfigContextsFilterParams({ status: ["true", "false"] })).toEqual({
      is_active: "true,false",
    });
  });
});
