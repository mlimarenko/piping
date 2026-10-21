import { describe, expect, it } from "vitest";

import {
  AUDIT_TABLE_CONFIG,
  auditSortParamForColumn,
  buildAuditFilterParams,
} from "./tableConfig";

describe("audit table config", () => {
  it("keeps audit sort and filter params declarative", () => {
    expect(AUDIT_TABLE_CONFIG.sortKeys).toEqual([
      "timestamp",
      "actor",
      "action",
      "target",
      "result",
      "summary",
    ]);
    expect(
      AUDIT_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([
      { colKey: "action", param: "event_type" },
      { colKey: "target", param: "target_type" },
      { colKey: "result", param: "result" },
    ]);
    expect(auditSortParamForColumn("timestamp")).toBe("created_at");
    expect(auditSortParamForColumn("summary")).toBe("summary");
    expect(
      buildAuditFilterParams({
        action: ["connection.session.created"],
        target: ["device", "site"],
        result: ["allowed"],
      }),
    ).toEqual({
      eventTypes: ["connection.session.created"],
      targetTypes: ["device", "site"],
      results: ["allowed"],
    });
  });
});
