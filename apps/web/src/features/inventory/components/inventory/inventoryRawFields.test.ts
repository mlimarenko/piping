import { describe, expect, it } from "vitest";
import {
  curatedSnakeKeys,
  customFieldRows,
  genericInspectorRows,
  humanizeKey,
  snakeKey,
} from "./inventoryRawFields";

describe("snakeKey", () => {
  it("converts camelCase Resource keys to snake_case backend keys", () => {
    expect(snakeKey("regionId")).toBe("region_id");
    expect(snakeKey("primaryIp")).toBe("primary_ip");
    expect(snakeKey("siteGroupName")).toBe("site_group_name");
    expect(snakeKey("slug")).toBe("slug");
  });
});

describe("humanizeKey", () => {
  it("title-cases snake keys and drops a trailing _id", () => {
    expect(humanizeKey("foo_bar")).toBe("Foo Bar");
    expect(humanizeKey("external_id")).toBe("External");
    expect(humanizeKey("custom_fields")).toBe("Custom Fields");
  });
});

describe("curatedSnakeKeys", () => {
  it("excludes each curated Sites field AND its FK id key", () => {
    const keys = curatedSnakeKeys("Sites", false);
    // display keys
    expect(keys.has("region")).toBe(true);
    expect(keys.has("tenant")).toBe(true);
    expect(keys.has("facility")).toBe(true);
    expect(keys.has("time_zone")).toBe(true);
    // FK id keys (so generic pass never re-renders region_id/tenant_id/…)
    expect(keys.has("region_id")).toBe(true);
    expect(keys.has("tenant_id")).toBe(true);
    expect(keys.has("site_group_id")).toBe(true);
    // structural
    expect(keys.has("id")).toBe(true);
    expect(keys.has("name")).toBe(true);
    expect(keys.has("status")).toBe(true);
  });

  it("adds host overview keys (region/cluster) only in host mode", () => {
    // region/cluster are NOT in the Devices CRUD form, so they only appear in
    // the rendered set when the host overview header is what shows them.
    expect(curatedSnakeKeys("Devices", false).has("region")).toBe(false);
    expect(curatedSnakeKeys("Devices", false).has("cluster")).toBe(false);
    const host = curatedSnakeKeys("Devices", true);
    expect(host.has("region")).toBe(true);
    expect(host.has("region_id")).toBe(true);
    expect(host.has("cluster")).toBe(true);
  });
});

describe("genericInspectorRows", () => {
  const rendered = curatedSnakeKeys("Sites", false);

  it("surfaces read-only fields the curated rows omit and dedups curated FK keys", () => {
    const rows = genericInspectorRows(
      {
        id: "x",
        name: "DC",
        region_id: "11111111-1111-7111-8111-111111111111", // curated → excluded
        slug: "shop1",
        external_id: "42",
      },
      rendered,
    );
    const byKey = Object.fromEntries(rows.map((r) => [r.key, r]));
    expect(byKey.region_id).toBeUndefined();
    expect(byKey.slug?.value).toBe("shop1");
    expect(byKey.slug?.isEmpty).toBe(false);
    expect(byKey.external_id?.value).toBe("42");
  });

  it("marks null/empty/empty-collection values as empty", () => {
    const rows = genericInspectorRows(
      { id: "x", facility_extra: "", time_zone2: null, asns: [], tags: [{ name: "core" }] },
      rendered,
    );
    const byKey = Object.fromEntries(rows.map((r) => [r.key, r]));
    expect(byKey.facility_extra?.isEmpty).toBe(true);
    expect(byKey.time_zone2?.isEmpty).toBe(true);
    expect(byKey.asns?.isEmpty).toBe(true);
    expect(byKey.tags?.isEmpty).toBe(false);
    expect(byKey.tags?.value).toBe("core");
  });

  it("keeps custom_fields out of generic rows and exposes them as standalone custom rows", () => {
    const generic = genericInspectorRows(
      { id: "x", custom_fields: { owner: "ops", tier: 1, empty: "" }, slug: "shop1" },
      rendered,
    );
    expect(generic.some((row) => row.key === "custom_fields")).toBe(false);
    expect(generic.some((row) => row.key === "slug")).toBe(true);

    expect(customFieldRows({ custom_fields: { owner: "ops", tier: 1, empty: "" } })).toEqual([
      { key: "owner", label: "Owner", value: "ops" },
      { key: "tier", label: "Tier", value: "1" },
    ]);
  });

  it("links a known FK UUID and skips an unknown FK UUID", () => {
    const rows = genericInspectorRows(
      {
        id: "x",
        manufacturer_id: "22222222-2222-7222-8222-222222222222", // known → link
        mystery_id: "33333333-3333-7333-8333-333333333333", // unknown → skipped (raw UUID noise)
      },
      rendered,
    );
    const byKey = Object.fromEntries(rows.map((r) => [r.key, r]));
    expect(byKey.manufacturer_id?.kind).toBe("link");
    expect(byKey.manufacturer_id?.relatedTab).toBe("Manufacturers");
    expect(byKey.mystery_id).toBeUndefined();
  });

  it("orders populated rows before empty ones", () => {
    const rows = genericInspectorRows(
      { id: "x", external_id: null, slug: "s" },
      rendered,
    );
    const slugIndex = rows.findIndex((r) => r.key === "slug");
    const externalIndex = rows.findIndex((r) => r.key === "external_id");
    expect(slugIndex).toBeLessThan(externalIndex);
    expect(rows[slugIndex]?.isEmpty).toBe(false);
    expect(rows[externalIndex]?.isEmpty).toBe(true);
  });
});
