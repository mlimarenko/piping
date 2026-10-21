import { describe, expect, it } from "vitest";

import {
  buildSyncMappingsFilterParams,
  SYNC_MAPPINGS_TABLE_CONFIG,
} from "./syncMappingsTableConfig";

describe("sync mappings table config", () => {
  it("keeps sync-mapping sort and filter params declarative", () => {
    expect(SYNC_MAPPINGS_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "versions",
      "direction",
      "provider_id",
    ]);
    expect(
      SYNC_MAPPINGS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([{ colKey: "direction", param: "direction" }]);
    expect(
      buildSyncMappingsFilterParams({
        direction: ["pull", "push"],
      }),
    ).toEqual({
      direction: ["pull", "push"],
    });
  });
});
