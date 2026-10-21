import { describe, expect, it } from "vitest";

import {
  EXPORTER_ASSIGNMENTS_TABLE_CONFIG,
} from "./tableConfig";

describe("observability table config", () => {
  it("keeps exporter assignment params declarative", () => {
    expect(EXPORTER_ASSIGNMENTS_TABLE_CONFIG.sortKeys).toEqual([
      "exporter",
      "scope_type",
      "scope_id",
      "job",
      "weight",
      "enabled",
    ]);
    expect(
      EXPORTER_ASSIGNMENTS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([
      { colKey: "exporter", param: "exporter_id" },
      { colKey: "scope_type", param: "scope_type" },
      { colKey: "scope_id", param: "scope_id" },
      { colKey: "enabled", param: "enabled" },
    ]);
  });
});
