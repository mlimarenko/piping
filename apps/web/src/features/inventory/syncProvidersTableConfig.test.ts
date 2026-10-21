import { describe, expect, it } from "vitest";

import {
  buildSyncProvidersFilterParams,
  SYNC_PROVIDERS_TABLE_CONFIG,
} from "./syncProvidersTableConfig";

describe("sync providers table config", () => {
  it("keeps provider list params declarative", () => {
    expect(SYNC_PROVIDERS_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "providerType",
      "enabled",
    ]);
    expect(SYNC_PROVIDERS_TABLE_CONFIG.sortKeyByColumn).toEqual({
      name: "name",
      providerType: "provider_type",
      enabled: "is_enabled",
    });
    expect(SYNC_PROVIDERS_TABLE_CONFIG.filterColumns).toEqual([
      { colKey: "providerType", param: "provider_type", serialize: expect.any(Function) },
      { colKey: "enabled", param: "is_enabled", serialize: expect.any(Function) },
    ]);
  });

  it("serializes provider filters as CSV params", () => {
    expect(
      buildSyncProvidersFilterParams({
        providerType: ["netbox", "custom"],
        enabled: ["true"],
        ignored: ["x"],
      }),
    ).toEqual({
      provider_type: "netbox,custom",
      is_enabled: "true",
    });
  });
});
