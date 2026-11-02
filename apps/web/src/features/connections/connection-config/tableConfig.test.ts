import { describe, expect, it } from "vitest";

import {
  buildConnectionConfigAssignmentFilterParams,
  buildConnectionConfigRecordFilterParams,
  CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG,
  CONNECTION_CONFIG_RECORDS_TABLE_CONFIG,
} from "./tableConfig";

describe("connection config table config", () => {
  it("keeps record sort and filter params declarative", () => {
    expect(CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "description",
      "protocol",
      "baseRecord",
      "enabled",
      "owner",
      "createdAt",
      "updatedAt",
    ]);
    expect(CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      name: "name",
      description: "description",
      protocol: "protocol",
      baseRecord: "base_record_id",
      enabled: "enabled",
      owner: "owner_user_id",
      createdAt: "created_at",
      updatedAt: "updated_at",
    });
    expect(
      CONNECTION_CONFIG_RECORDS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([
      { colKey: "protocol", param: "protocol" },
      { colKey: "baseRecord", param: "base_record_id" },
      { colKey: "enabled", param: "enabled" },
      { colKey: "owner", param: "owner_user_id" },
    ]);
    expect(
      buildConnectionConfigRecordFilterParams({
        protocol: ["ssh", "rdp"],
        enabled: ["true"],
        owner: ["user-1"],
      }),
    ).toEqual({
      protocol: "ssh,rdp",
      enabled: "true",
      owner_user_id: "user-1",
    });
  });

  it("keeps assignment sort and filter params declarative", () => {
    expect(CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG.sortKeys).toEqual([
      "scopeType",
      "scope",
      "weight",
      "enabled",
      "createdAt",
    ]);
    expect(CONNECTION_CONFIG_ASSIGNMENTS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      scopeType: "scope_type",
      scope: "scope_id",
      weight: "weight",
      enabled: "enabled",
      createdAt: "created_at",
    });
    expect(
      buildConnectionConfigAssignmentFilterParams({
        scopeType: ["site"],
        scope: ["scope-1", "scope-2"],
        enabled: ["false"],
      }),
    ).toEqual({
      scope_type: "site",
      scope_id: "scope-1,scope-2",
      enabled: "false",
    });
  });
});
