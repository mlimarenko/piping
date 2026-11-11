import { describe, expect, it } from "vitest";

import {
  buildConfigContextBindingsFilterParams,
  CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG,
} from "./configContextBindingsTableConfig";

describe("config-context bindings table config", () => {
  it("keeps config-context binding filters declarative", () => {
    expect(CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG.sortKeys).toEqual(["scope_type", "scope_id"]);
    expect(
      CONFIG_CONTEXT_BINDINGS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([{ colKey: "scope_type", param: "scope_type" }]);
    expect(buildConfigContextBindingsFilterParams({ scope_type: ["device", "vm"] })).toEqual({
      scope_type: "device,vm",
    });
  });
});
