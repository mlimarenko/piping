import { describe, expect, it } from "vitest";

import {
  GRANTS_TABLE_CONFIG,
  GROUPS_TABLE_CONFIG,
  POLICIES_TABLE_CONFIG,
  ROLES_TABLE_CONFIG,
  USERS_TABLE_CONFIG,
} from "./tableConfig";

describe("access-control table sort config", () => {
  it("matches the backend sort allowlists", () => {
    expect(USERS_TABLE_CONFIG.sortKeys).toEqual(["username", "email", "status", "roles", "auth_source"]);
    expect(GROUPS_TABLE_CONFIG.sortKeys).toEqual(["name", "external_id", "assignments"]);
    expect(ROLES_TABLE_CONFIG.sortKeys).toEqual(["name", "description", "policyCount"]);
    expect(POLICIES_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "effect",
      "enabled",
      "subjects",
      "policy_actions",
      "scopes",
    ]);
    expect(GRANTS_TABLE_CONFIG.sortKeys).toEqual(["role", "subject", "type"]);
  });

  it("keeps the grant type column filterable and sortable through the subject discriminator", () => {
    expect(GRANTS_TABLE_CONFIG.sortKeys).toContain("type");
    expect(GRANTS_TABLE_CONFIG.filterColumns).toContainEqual(
      expect.objectContaining({ colKey: "type", param: "subject_type", type: "enum" }),
    );
  });

  it("keeps the grant subject filter inside the shared table config", () => {
    const subjectFilter = GRANTS_TABLE_CONFIG.filterColumns.find((column) => column.colKey === "subject");

    expect(subjectFilter).toBeDefined();
    expect(subjectFilter).toMatchObject({ colKey: "subject", type: "fk" });
    if (!subjectFilter || subjectFilter.type !== "fk" || !subjectFilter.buildParams) {
      throw new Error("subject filter contract missing");
    }

    expect(
      subjectFilter.buildParams(
        ["user-1", "group-1", "user-2"],
        { subjectUserIds: new Set(["user-1", "user-2"]) },
      ),
    ).toEqual({
      user_id: ["user-1", "user-2"],
      group_id: ["group-1"],
    });
  });
});
