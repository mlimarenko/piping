import { describe, it, expect } from "vitest";
import type { WebhookEventCatalogItem } from "@/features/webhooks/api";
import {
  computeEventPresets,
  deserializeConditions,
  emptyConditionRow,
  filterCatalog,
  groupCatalog,
  isBundleActive,
  isStructuredConditionSet,
  parseConditionValue,
  serializeConditions,
  stringifyConditionValue,
  toggleEventBundle,
  type ConditionRow,
} from "./webhookEvents";

const ev = (group: string, object: string, action: string): WebhookEventCatalogItem => ({
  group,
  object,
  action,
  value: `${group}.${object}.${action}`,
});

/** A representative slice of the catalog spanning two groups. */
const catalog: WebhookEventCatalogItem[] = [
  ev("inventory", "tenant", "created"),
  ev("inventory", "tenant", "updated"),
  ev("inventory", "tenant", "deleted"),
  ev("inventory", "host", "created"),
  ev("inventory", "host", "deleted"),
  ev("connections", "session", "activated"),
  ev("connections", "session", "disconnected"),
  ev("connections", "session", "failed"),
];

describe("groupCatalog", () => {
  it("groups by group then object, in stable order", () => {
    const groups = groupCatalog(catalog);
    expect(groups.map((g) => g.group)).toEqual(["inventory", "connections"]);
    const inventory = groups.find((g) => g.group === "inventory");
    expect(inventory?.objects.map((o) => o.object)).toEqual(["host", "tenant"]);
  });

  it("sorts actions inside an object by the action order", () => {
    const inventory = groupCatalog(catalog).find((g) => g.group === "inventory");
    const tenant = inventory?.objects.find((o) => o.object === "tenant");
    expect(tenant?.items.map((i) => i.action)).toEqual(["created", "updated", "deleted"]);
  });

  it("flattens every value of a group", () => {
    const connections = groupCatalog(catalog).find((g) => g.group === "connections");
    expect(connections?.values).toEqual([
      "connections.session.activated",
      "connections.session.disconnected",
      "connections.session.failed",
    ]);
  });

  it("returns an empty list for an empty catalog", () => {
    expect(groupCatalog([])).toEqual([]);
  });
});

describe("filterCatalog", () => {
  it("returns the catalog unchanged for an empty term", () => {
    expect(filterCatalog(catalog, "  ")).toHaveLength(catalog.length);
  });

  it("matches object, action, group, and raw value case-insensitively", () => {
    expect(filterCatalog(catalog, "TENANT").every((i) => i.object === "tenant")).toBe(true);
    expect(filterCatalog(catalog, "deleted").every((i) => i.action === "deleted")).toBe(true);
    expect(filterCatalog(catalog, "connections.session.failed")).toHaveLength(1);
  });
});

describe("computeEventPresets", () => {
  it("returns an empty list for an empty catalog", () => {
    expect(computeEventPresets([])).toEqual([]);
  });

  it('always offers an "all" preset covering every event', () => {
    const all = computeEventPresets(catalog).find((p) => p.kind === "all");
    expect(all?.values).toHaveLength(catalog.length);
  });

  it("offers per-action presets only when more than one event shares the action", () => {
    const presets = computeEventPresets(catalog);
    const created = presets.find((p) => p.kind === "action" && p.token === "created");
    expect(created?.values).toEqual([
      "inventory.tenant.created",
      "inventory.host.created",
    ]);
    // `updated` only exists once → no preset.
    expect(presets.some((p) => p.kind === "action" && p.token === "updated")).toBe(false);
  });

  it("offers per-group and per-object presets for multi-event bundles", () => {
    const presets = computeEventPresets(catalog);
    expect(presets.some((p) => p.kind === "group" && p.token === "inventory")).toBe(true);
    expect(presets.some((p) => p.kind === "object" && p.token === "tenant")).toBe(true);
    expect(presets.some((p) => p.kind === "object" && p.token === "session")).toBe(true);
  });

  it("orders presets all → action → group → object", () => {
    const kinds = computeEventPresets(catalog).map((p) => p.kind);
    const firstGroup = kinds.indexOf("group");
    const firstObject = kinds.indexOf("object");
    expect(kinds[0]).toBe("all");
    expect(kinds.indexOf("action")).toBeLessThan(firstGroup);
    expect(firstGroup).toBeLessThan(firstObject);
  });
});

describe("isBundleActive / toggleEventBundle", () => {
  const values = ["a", "b", "c"];

  it("isBundleActive is false for an empty bundle", () => {
    expect(isBundleActive([], new Set(["a"]))).toBe(false);
  });

  it("isBundleActive requires every value present", () => {
    expect(isBundleActive(values, new Set(["a", "b"]))).toBe(false);
    expect(isBundleActive(values, new Set(["a", "b", "c", "d"]))).toBe(true);
  });

  it("adds missing values when not all present, preserving the rest", () => {
    expect(toggleEventBundle(values, ["a", "x"])).toEqual(["a", "x", "b", "c"]);
  });

  it("removes the whole bundle when fully active", () => {
    expect(toggleEventBundle(values, ["a", "b", "c", "x"])).toEqual(["x"]);
  });
});

describe("parseConditionValue", () => {
  it("parses numbers, booleans, and null", () => {
    expect(parseConditionValue("42")).toBe(42);
    expect(parseConditionValue("-3.5")).toBe(-3.5);
    expect(parseConditionValue("true")).toBe(true);
    expect(parseConditionValue("false")).toBe(false);
    expect(parseConditionValue("null")).toBe(null);
  });

  it("parses JSON arrays and objects", () => {
    expect(parseConditionValue('["a","b"]')).toEqual(["a", "b"]);
    expect(parseConditionValue('{"k":1}')).toEqual({ k: 1 });
  });

  it("keeps bare words as strings", () => {
    expect(parseConditionValue("pending")).toBe("pending");
    expect(parseConditionValue("1.2.3")).toBe("1.2.3");
  });

  it("unwraps an explicitly quoted string", () => {
    expect(parseConditionValue('"true"')).toBe("true");
  });

  it("falls back to the raw text when bracketed JSON is malformed", () => {
    expect(parseConditionValue("[not json")).toBe("[not json");
  });

  it("returns empty string for blank input", () => {
    expect(parseConditionValue("   ")).toBe("");
  });
});

describe("stringifyConditionValue", () => {
  it("round-trips primitives and structures", () => {
    expect(stringifyConditionValue("pending")).toBe("pending");
    expect(stringifyConditionValue(42)).toBe("42");
    expect(stringifyConditionValue(true)).toBe("true");
    expect(stringifyConditionValue(null)).toBe("null");
    expect(stringifyConditionValue(["a", "b"])).toBe('["a","b"]');
  });
});

describe("serializeConditions", () => {
  const row = (attr: string, op: ConditionRow["op"], value: string): ConditionRow => ({
    ...emptyConditionRow(),
    attr,
    op,
    value,
  });

  it("omits conditions entirely when no row is complete", () => {
    expect(serializeConditions("and", [])).toBeUndefined();
    expect(serializeConditions("and", [row("", "eq", "x")])).toBeUndefined();
  });

  it("serializes complete rows under the chosen logic with parsed values", () => {
    const out = serializeConditions("or", [
      row("status", "eq", "active"),
      row("attempts", "gte", "3"),
    ]);
    expect(out).toEqual({
      or: [
        { attr: "status", op: "eq", value: "active" },
        { attr: "attempts", op: "gte", value: 3 },
      ],
    });
  });

  it("drops incomplete rows but keeps complete ones", () => {
    const out = serializeConditions("and", [
      row("status", "eq", "active"),
      row("", "ne", "ignored"),
    ]);
    expect(out).toEqual({ and: [{ attr: "status", op: "eq", value: "active" }] });
  });
});

describe("deserializeConditions", () => {
  it("returns an empty AND set for nullish / non-object input", () => {
    expect(deserializeConditions(null)).toEqual({ logic: "and", rows: [] });
    expect(deserializeConditions("nope")).toEqual({ logic: "and", rows: [] });
    expect(deserializeConditions(undefined)).toEqual({ logic: "and", rows: [] });
  });

  it("reads the logic and the leaf rows", () => {
    const { logic, rows } = deserializeConditions({
      or: [
        { attr: "status", op: "eq", value: "active" },
        { attr: "n", op: "gt", value: 5 },
      ],
    });
    expect(logic).toBe("or");
    expect(rows.map((r) => ({ attr: r.attr, op: r.op, value: r.value }))).toEqual([
      { attr: "status", op: "eq", value: "active" },
      { attr: "n", op: "gt", value: "5" },
    ]);
  });

  it("defaults an unknown op back to eq", () => {
    const { rows } = deserializeConditions({ and: [{ attr: "a", op: "weird", value: 1 }] });
    expect(rows[0]?.op).toBe("eq");
  });

  it("round-trips through serialize", () => {
    const original = {
      and: [
        { attr: "status", op: "eq", value: "active" },
        { attr: "count", op: "gte", value: 3 },
      ],
    };
    const { logic, rows } = deserializeConditions(original);
    expect(serializeConditions(logic, rows)).toEqual(original);
  });
});

describe("isStructuredConditionSet", () => {
  it("recognizes and/or sets with attr leaves", () => {
    expect(isStructuredConditionSet({ and: [{ attr: "a", op: "eq", value: 1 }] })).toBe(true);
    expect(isStructuredConditionSet({ or: [] })).toBe(true);
  });

  it("rejects shapes the structured editor cannot host", () => {
    expect(isStructuredConditionSet(null)).toBe(false);
    expect(isStructuredConditionSet({ and: "x" })).toBe(false);
    expect(isStructuredConditionSet({ and: [{ op: "eq" }] })).toBe(false);
    expect(isStructuredConditionSet({ nested: { and: [] } })).toBe(false);
  });
});
