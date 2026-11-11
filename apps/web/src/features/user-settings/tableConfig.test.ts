import { describe, expect, it } from "vitest";

import {
  buildUserCredentialFilterParams,
  buildUserTokenFilterParams,
  USER_CREDENTIALS_TABLE_CONFIG,
  USER_TOKENS_TABLE_CONFIG,
} from "./tableConfig";

describe("user settings table config", () => {
  it("keeps personal-credential filters declarative", () => {
    expect(USER_CREDENTIALS_TABLE_CONFIG.sortKeys).toEqual(["protocol", "name", "isDefault"]);
    expect(USER_CREDENTIALS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      protocol: "protocol",
      name: "name",
      isDefault: "is_default",
    });
    expect(USER_CREDENTIALS_TABLE_CONFIG.filterColumns).toEqual([
      { colKey: "protocol", param: "protocol" },
      { colKey: "isDefault", param: "is_default" },
    ]);
    expect(
      buildUserCredentialFilterParams({
        protocol: ["RDP"],
        isDefault: ["true"],
      }),
    ).toEqual({
      protocol: ["RDP"],
      is_default: ["true"],
    });
  });

  it("keeps token filters declarative", () => {
    expect(USER_TOKENS_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "scopes",
      "status",
      "createdAt",
      "expiresAt",
    ]);
    expect(USER_TOKENS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      name: "name",
      scopes: "scopes",
      status: "status",
      createdAt: "created_at",
      expiresAt: "expires_at",
    });
    expect(USER_TOKENS_TABLE_CONFIG.filterColumns).toEqual([
      { colKey: "scopes", param: "scope" },
      { colKey: "status", param: "status" },
    ]);
    expect(
      buildUserTokenFilterParams({
        scopes: ["read:inventory"],
        status: ["active"],
      }),
    ).toEqual({
      scope: ["read:inventory"],
      status: ["active"],
    });
  });
});
