import { describe, expect, it } from "vitest";

import {
  PROXY_ASSIGNMENTS_TABLE_CONFIG,
  PROXY_NODES_TABLE_CONFIG,
} from "./proxyTableConfig";
import { buildDataTableFilterParams } from "@/shared/ui-shared/tableState";

describe("proxy table config", () => {
  it("keeps proxy node sort and filter params declarative", () => {
    expect(PROXY_NODES_TABLE_CONFIG.sortKeys).toEqual(["name", "slug", "baseUrl", "enabled"]);
    expect(PROXY_NODES_TABLE_CONFIG.sortKeyByColumn).toEqual({
      name: "name",
      slug: "slug",
      baseUrl: "base_url",
      enabled: "enabled",
    });
    expect(
      PROXY_NODES_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([{ colKey: "enabled", param: "enabled" }]);
    expect(buildDataTableFilterParams({ enabled: ["true"] }, PROXY_NODES_TABLE_CONFIG.filterColumns)).toEqual({ enabled: "true" });
    expect(buildDataTableFilterParams({ enabled: ["true", "false"] }, PROXY_NODES_TABLE_CONFIG.filterColumns)).toEqual({
      enabled: "true,false",
    });
  });

  it("keeps proxy assignment params declarative", () => {
    expect(PROXY_ASSIGNMENTS_TABLE_CONFIG.sortKeys).toEqual([
      "proxy",
      "scope_type",
      "weight",
      "enabled",
    ]);
    expect(PROXY_ASSIGNMENTS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      proxy: "proxy_node_id",
      scope_type: "scope_type",
      weight: "weight",
      enabled: "enabled",
    });
    expect(
      buildDataTableFilterParams({
        proxy: ["proxy-1", "proxy-2"],
        scope_type: ["site"],
        enabled: ["false"],
      }, PROXY_ASSIGNMENTS_TABLE_CONFIG.filterColumns),
    ).toEqual({
      proxy_node_id: "proxy-1,proxy-2",
      scope_type: "site",
      enabled: "false",
    });
  });
});
