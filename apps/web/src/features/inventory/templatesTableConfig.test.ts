import { describe, expect, it } from "vitest";

import {
  INVENTORY_TEMPLATES_TABLE_CONFIG,
  INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG,
} from "./templatesTableConfig";

describe("inventory templates table config", () => {
  it("keeps inventory template params declarative", () => {
    expect(INVENTORY_TEMPLATES_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "slug",
      "shape",
      "status",
      "updatedAt",
    ]);
    expect(INVENTORY_TEMPLATES_TABLE_CONFIG.filterColumns).toEqual([
      { colKey: "status", param: "status", serialize: expect.any(Function) },
    ]);
  });

  it("keeps recent-runs sort keys declarative", () => {
    expect(INVENTORY_TEMPLATE_RUNS_TABLE_CONFIG.sortKeys).toEqual([
      "status",
      "template",
      "startedAt",
      "duration",
      "steps",
    ]);
  });
});
