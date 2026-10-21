import { describe, expect, it } from "vitest";

import {
  buildConstraints,
  parseConstraints,
  type AttributeConditionRow,
} from "./constraints";

const row = (key: string, value: string): AttributeConditionRow => ({
  id: `${key}:${value}`,
  key,
  value,
});

/** Compare parsed rows ignoring their non-deterministic ids. */
const asPairs = (rows: AttributeConditionRow[]) => rows.map(({ key, value }) => ({ key, value }));

describe("buildConstraints", () => {
  it("returns undefined when there are no usable rows", () => {
    expect(buildConstraints([])).toBeUndefined();
    // Empty key / empty value rows are ignored.
    expect(buildConstraints([row("", "x"), row("role_id", "")])).toBeUndefined();
  });

  it("emits a bare attributes node for a single equality row", () => {
    expect(buildConstraints([row("role_id", "abc")])).toEqual({
      attributes: { role_id: "abc" },
    });
  });

  it("ANDs distinct keys — the P4 scenario (role_id=X AND protocol=vnc)", () => {
    expect(
      buildConstraints([row("role_id", "role-x"), row("protocol", "vnc")]),
    ).toEqual({
      and: [{ attributes: { role_id: "role-x" } }, { attributes: { protocol: "vnc" } }],
    });
  });

  it("ORs same-key rows — role_id in {A,B}", () => {
    expect(buildConstraints([row("role_id", "A"), row("role_id", "B")])).toEqual({
      or: [{ attributes: { role_id: "A" } }, { attributes: { role_id: "B" } }],
    });
  });

  it("combines an OR group with another key under AND", () => {
    expect(
      buildConstraints([
        row("role_id", "A"),
        row("role_id", "B"),
        row("protocol", "vnc"),
      ]),
    ).toEqual({
      and: [
        { or: [{ attributes: { role_id: "A" } }, { attributes: { role_id: "B" } }] },
        { attributes: { protocol: "vnc" } },
      ],
    });
  });

  it("de-duplicates identical value on the same key", () => {
    expect(buildConstraints([row("site_id", "s1"), row("site_id", "s1")])).toEqual({
      attributes: { site_id: "s1" },
    });
  });

  it("preserves opaque extra nodes, ANDed after attribute nodes", () => {
    const ipRange = { ip_range: "10.0.0.0/8" };
    expect(buildConstraints([row("protocol", "ssh")], [ipRange])).toEqual({
      and: [{ attributes: { protocol: "ssh" } }, ipRange],
    });
  });

  it("re-emits a lone extra node unwrapped", () => {
    const timeWindow = { time_window: { start: "08:00", end: "18:00" } };
    expect(buildConstraints([], [timeWindow])).toEqual(timeWindow);
  });
});

describe("parseConstraints", () => {
  it("returns empty for null/undefined", () => {
    expect(parseConstraints(undefined)).toEqual({ rows: [], extra: [] });
    expect(parseConstraints(null)).toEqual({ rows: [], extra: [] });
  });

  it("parses a bare attributes node into one row per key", () => {
    const parsed = parseConstraints({ attributes: { role_id: "abc", protocol: "vnc" } });
    expect(asPairs(parsed.rows)).toEqual([
      { key: "role_id", value: "abc" },
      { key: "protocol", value: "vnc" },
    ]);
    expect(parsed.extra).toEqual([]);
  });

  it("parses an AND of attributes into flat rows", () => {
    const parsed = parseConstraints({
      and: [{ attributes: { role_id: "role-x" } }, { attributes: { protocol: "vnc" } }],
    });
    expect(asPairs(parsed.rows)).toEqual([
      { key: "role_id", value: "role-x" },
      { key: "protocol", value: "vnc" },
    ]);
  });

  it("parses a same-key OR into repeated rows", () => {
    const parsed = parseConstraints({
      or: [{ attributes: { role_id: "A" } }, { attributes: { role_id: "B" } }],
    });
    expect(asPairs(parsed.rows)).toEqual([
      { key: "role_id", value: "A" },
      { key: "role_id", value: "B" },
    ]);
  });

  it("routes non-attribute nodes to extra", () => {
    const parsed = parseConstraints({
      and: [{ attributes: { protocol: "ssh" } }, { ip_range: "10.0.0.0/8" }],
    });
    expect(asPairs(parsed.rows)).toEqual([{ key: "protocol", value: "ssh" }]);
    expect(parsed.extra).toEqual([{ ip_range: "10.0.0.0/8" }]);
  });

  it("does not coerce non-string attribute values — preserves the node in extra", () => {
    const node = { attributes: { count: 3 } };
    const parsed = parseConstraints(node);
    expect(parsed.rows).toEqual([]);
    expect(parsed.extra).toEqual([node]);
  });
});

describe("round-trip", () => {
  it("parse(build(rows)) preserves the rows (P4)", () => {
    const rows = [row("role_id", "role-x"), row("protocol", "vnc")];
    const built = buildConstraints(rows);
    expect(asPairs(parseConstraints(built).rows)).toEqual([
      { key: "role_id", value: "role-x" },
      { key: "protocol", value: "vnc" },
    ]);
  });

  it("build(parse(x)) keeps an unmodeled ip_range node", () => {
    const original = {
      and: [{ attributes: { protocol: "vnc" } }, { ip_range: "192.168.0.0/16" }],
    };
    const { rows, extra } = parseConstraints(original);
    expect(buildConstraints(rows, extra)).toEqual(original);
  });

  it("round-trips a same-key OR through build/parse", () => {
    const rows = [row("site_id", "A"), row("site_id", "B")];
    const parsed = parseConstraints(buildConstraints(rows));
    expect(asPairs(parsed.rows)).toEqual([
      { key: "site_id", value: "A" },
      { key: "site_id", value: "B" },
    ]);
  });
});
