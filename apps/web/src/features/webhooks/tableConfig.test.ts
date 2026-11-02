import { describe, expect, it } from "vitest";

import { buildWebhookFilterParams, WEBHOOKS_TABLE_CONFIG } from "./tableConfig";

describe("webhooks table config", () => {
  it("keeps webhook sort and filter params declarative", () => {
    expect(WEBHOOKS_TABLE_CONFIG.sortKeys).toEqual([
      "name",
      "target_url",
      "secret_set",
      "event_types",
      "enabled",
      "last_fired_at",
      "last_status",
    ]);
    expect(
      WEBHOOKS_TABLE_CONFIG.filterColumns.map((column) => ({
        colKey: column.colKey,
        param: column.param,
      })),
    ).toEqual([
      { colKey: "enabled", param: "enabled" },
      { colKey: "secret_set", param: "secret_set" },
      { colKey: "event_types", param: "event_types" },
      { colKey: "last_status", param: "last_status" },
    ]);
    expect(
      buildWebhookFilterParams({
        enabled: ["true"],
        event_types: ["inventory.created", "inventory.updated"],
      }),
    ).toEqual({
      enabled: "true",
      event_types: "inventory.created,inventory.updated",
    });
  });
});
