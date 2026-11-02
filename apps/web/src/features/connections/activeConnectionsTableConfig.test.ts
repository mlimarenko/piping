import { describe, expect, it } from "vitest";

import {
  ACTIVE_CONNECTIONS_TABLE_CONFIG,
} from "./activeConnectionsTableConfig";
import { buildDataTableFilterParams } from "@/shared/ui-shared/tableState";

describe("active connections table config", () => {
  it("keeps active-session sort and filter params declarative", () => {
    expect(ACTIVE_CONNECTIONS_TABLE_CONFIG.sortKeys).toEqual([
      "host",
      "user",
      "protocol",
      "status",
      "started_at",
      "lastActivity",
    ]);
    expect(ACTIVE_CONNECTIONS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      host: "host",
      user: "user",
      protocol: "protocol",
      status: "status",
      started_at: "started_at",
      lastActivity: "last_activity",
    });
    expect(
      ACTIVE_CONNECTIONS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([
      { colKey: "host", param: "target_id" },
      { colKey: "user", param: "user_id" },
      { colKey: "protocol", param: "protocol" },
      { colKey: "status", param: "status" },
    ]);
    expect(
      buildDataTableFilterParams({
        host: ["target-1"],
        user: ["user-1"],
        protocol: ["ssh", "rdp"],
        status: ["active"],
      }, ACTIVE_CONNECTIONS_TABLE_CONFIG.filterColumns),
    ).toEqual({
      target_id: ["target-1"],
      user_id: ["user-1"],
      protocol: ["ssh", "rdp"],
      status: ["active"],
    });
  });
});
